#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of security/help-session: it plays the owner at a
# terminal and the helper, both through ssh, which is how a person on
# another computer would reach the machine (ADR 0540). The owner signs in
# with the lab key, as the lab account, and starts a help session in a
# terminal of sshd's; the helper signs in as help with the code and no
# key. What each says is kept in a file of the test's folder, to look in,
# and what each types goes through a pipe the script holds open.
#
#   help-session.sh MACHINE PORT
#
# MACHINE is the machine's address and PORT the lab PC's port that
# reaches its SSH, as {machine} and {machine-port:22} give them. The
# lab account's password is read from the standard input. Each line the
# script prints says what was seen; the first thing that is not stops it,
# with what the owner's and the helper's terminals said. It runs in the
# test's own folder.
set -eu

machine="$1"
port="$2"
IFS= read -r password
lab_key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"
work=$(pwd)
hosts="$work/known_hosts"
options="-F /dev/null -p $port -o StrictHostKeyChecking=yes -o UserKnownHostsFile=$hosts -o LogLevel=ERROR -o ConnectTimeout=30"

# The machine's host key as it shows itself now, before anything is
# trusted: the one the owner's screen must show the fingerprint of.
ssh-keyscan -T 10 -t ed25519 -p "$port" "$machine" > "$hosts" 2> /dev/null
if [ ! -s "$hosts" ]; then
	echo "no host key: ssh-keyscan read none from the machine"
	exit 1
fi
scanned=$(ssh-keygen -lf "$hosts" | awk '{ print $2 }')

# An askpass program that answers a password prompt with what the file
# `ask` holds: the helper's code. The owner signs in with the lab key and
# is never asked.
cat > askpass <<'ASKPASS'
#!/bin/sh
cat "$ASK"
ASKPASS
chmod 700 askpass
export ASK="$work/ask" SSH_ASKPASS="$work/askpass" SSH_ASKPASS_REQUIRE=force

# Whatever ends, what the script started ends too.
cleanup() {
	for pid in $(cat ./*.pid 2> /dev/null); do
		kill "$pid" 2> /dev/null || true
	done
}
trap cleanup EXIT

# Without the escape sequences and carriage returns a terminal's output
# has, which grep would trip on.
plain() {
	sed -e 's/\x1b\[[0-9;?]*[a-zA-Z]//g' -e 's/\r//g' "$1.out"
}

# Starts `ssh ARGS...` as the terminal NAME: its output is NAME.out, and
# what is written to NAME.in is what its person types. A process holds
# the pipe open, so that ssh does not see the end of its input.
terminal() {
	name="$1"
	shift
	rm -f "$name.in" "$name.out"
	mkfifo "$name.in"
	# shellcheck disable=SC2086 # the options are words
	TERM=dumb ssh $options "$@" < "$name.in" > "$name.out" 2>&1 &
	echo $! > "$name.ssh.pid"
	sleep 600 > "$name.in" &
	echo $! > "$name.hold.pid"
}

# Types TEXT at the terminal NAME.
type_at() {
	printf '%s' "$2" > "$1.in"
}

# Waits up to SECONDS until the terminal NAME has written PATTERN (a
# fixed string).
sees() {
	name="$1"
	pattern="$2"
	left="$3"
	while [ "$left" -gt 0 ]; do
		if plain "$name" | grep -q -F -- "$pattern"; then
			return 0
		fi
		sleep 1
		left=$((left - 1))
	done
	echo "$name never said: $pattern"
	echo "--- $name said:"
	plain "$name" | tail -n 25
	exit 1
}

# Whether the process PID runs: one that ended and was not waited for
# is a zombie, which kill -0 still finds.
alive() {
	[ -r "/proc/$1/status" ] && ! grep -q '^State:[[:space:]]*Z' "/proc/$1/status"
}

# Waits up to SECONDS until the terminal NAME has ended.
ends() {
	pid=$(cat "$1.ssh.pid")
	left="$2"
	while alive "$pid"; do
		if [ "$left" -le 0 ]; then
			echo "$1 did not end"
			plain "$1" | tail -n 25
			exit 1
		fi
		sleep 1
		left=$((left - 1))
	done
}

# Says `LABEL: refused` when signing in as help with the code CODE ($2)
# is refused, as ssh ends with 255 and says permission was denied, and
# stops the script when it is not.
refused() {
	printf '%s\n' "$2" > ask
	set +e
	ssh $options -o PubkeyAuthentication=no -o PreferredAuthentications=password \
		-o NumberOfPasswordPrompts=1 help@"$machine" true < /dev/null > refused.out 2>&1
	status=$?
	set -e
	if [ "$status" -eq 255 ] && grep -q 'Permission denied' refused.out; then
		echo "$1: refused"
	else
		echo "$1: not refused (status $status)"
		cat refused.out
		exit 1
	fi
}

# The owner starts a help session. A key signs the lab account in, as it
# does for every lab step; the session's screen is the terminal's.
terminal owner -i "$lab_key" -o IdentitiesOnly=yes -o BatchMode=yes -tt "lab@$machine" help-session start
sees owner "Waiting for your helper..." 60
sees owner "Give your helper:" 5
sees owner "ssh help@" 5
echo "started"

# sshd's host key as the owner's screen shows it is the one the lab PC
# read from the machine's port.
shown=$(plain owner | awk '$1 ~ /^SHA256:/ { print $1; exit }')
if [ "$shown" = "$scanned" ]; then
	echo "host key: the same"
else
	echo "host key: the screen shows '$shown', ssh-keyscan read '$scanned'"
	exit 1
fi

# The code: 16 letters and digits, in groups of four with dashes.
code=$(plain owner | awk '$1 == "the" && $2 == "code" { print $3; exit }')
digits=$(printf '%s' "$code" | tr -d '-')
case "$code" in
[A-Z2-9][A-Z2-9][A-Z2-9][A-Z2-9]-[A-Z2-9][A-Z2-9][A-Z2-9][A-Z2-9]-[A-Z2-9][A-Z2-9][A-Z2-9][A-Z2-9]-[A-Z2-9][A-Z2-9][A-Z2-9][A-Z2-9])
	echo "code: 16 characters"
	;;
*)
	echo "code: the screen shows '$code'"
	plain owner | tail -n 25
	exit 1
	;;
esac

# A wrong code is refused; the right one signs the helper in as help.
refused "wrong code" "AAAA-AAAA-AAAA-AAAA"
printf '%s\n' "$digits" > ask
terminal helper -o PubkeyAuthentication=no -o PreferredAuthentications=password \
	-o NumberOfPasswordPrompts=1 -tt "help@$machine"
sees helper "You are in a help session" 60
sees owner "Your helper signed in from" 30
echo "helper: signed in"

# What the helper types runs in the owner's shell, so that the file is
# the lab account's, in its home.
type_at helper 'echo helped > ~/proof
'
type_at helper 'echo "mine $(stat -c %U ~/proof) $(cat ~/proof)"
'
sees helper "mine lab helped" 60
sees owner "mine lab helped" 30
echo "typed: the file is the owner's"

# The code is spent.
refused "same code again" "$digits"

# elevate says what allows it.
type_at helper 'elevate true; echo "elevate-status $?"
'
sees helper "elevate-status 1" 60
sees helper "elevate runs only while its owner allows it: help-session allow-elevate" 5
echo "elevate: refused"

# The owner allows it from a second sign-in, which asks the password.
terminal allow -i "$lab_key" -o IdentitiesOnly=yes -o BatchMode=yes -tt "lab@$machine" help-session allow-elevate
sees allow "password for lab:" 60
type_at allow "$password
"
sees allow "elevate runs in your help screen for 15 minutes" 60
sees owner "elevate runs in the help screen for 15 minutes" 30
ends allow 30
echo "elevate: allowed"
type_at helper 'elevate sh -c '\''echo "elev $(id -un)"'\''
'
sees helper "elev root" 60
echo "elevate: runs as root"

# Left running, until the owner ends the session with Ctrl-]. The job's
# process is named sleep only once it has started the program, so the
# count waits for it, up to 20 seconds.
type_at helper 'sleep 999 &
'
type_at helper 'n=0; until pgrep -x sleep > /dev/null || [ $n -ge 20 ]; do n=$((n+1)); sleep 1; done; echo "bg $(pgrep -c -x sleep)"
'
sees helper "bg 1" 60
echo "sleep: left running"
printf '\035' > owner.in
sees helper "[help-session] The help session ended." 60
ends helper 30
echo "end: the helper was told"
sees owner "ended: the owner ended it." 30
id=$(plain owner | awk '$1 == "Help" && $2 == "session" && /ended: the owner ended it\./ { print $3; exit }')
if [ -z "$id" ]; then
	echo "end: the owner's screen names no session"
	plain owner | tail -n 25
	exit 1
fi
echo "id: $id"

# The lab account's own commands, with no terminal.
as_lab() {
	# shellcheck disable=SC2086 # the options are words
	ssh $options -i "$lab_key" -o IdentitiesOnly=yes -o BatchMode=yes "lab@$machine" "$@"
}

# A second session, with no helper, ended from another terminal of the
# owner's, as the owner ends it who is not at the session's own screen.
terminal second -i "$lab_key" -o IdentitiesOnly=yes -o BatchMode=yes -tt "lab@$machine" help-session start
sees second "Waiting for your helper..." 60
as_lab 'help-session end' | tr -d '\r'
sees second "ended: the owner ended it." 30
ends second 30
echo "second: ended from another terminal"
as_lab 'echo "proof $(cat ~/proof) $(stat -c %U ~/proof)"; echo "sleepers $(pgrep -c -x -r RSDT sleep)"'

# The list has the session, ended; its transcript has what the helper
# typed and when elevate was allowed.
as_lab 'help-session log' | tr -s ' ' > list.txt
cat list.txt
as_lab "help-session log $id" | tr -s ' ' > transcript.txt
cat transcript.txt
echo "help-session-end"
