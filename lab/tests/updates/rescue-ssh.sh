#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The lab PC's side of updates/rescue-ssh: it reads the machine's SSH host
# key while the installed system runs, and, once Rescue has started with
# SSH on, signs the lab account in to Rescue with the lab key as one of
# the system's administrators would (ADR 0540).
#
#   rescue-ssh.sh scan MACHINE PORT     keeps the host key, says its fingerprint
#   rescue-ssh.sh menu MACHINE PORT     signs in, checks the host key, is refused
#                                       with a password and with a command
#   rescue-ssh.sh hold MACHINE PORT     takes the lock at copying the files off,
#                                       waits for the release file, answers
#   rescue-ssh.sh release               tells hold to answer, waits until it did
#
# MACHINE is the machine's address and PORT the lab PC's port that
# reaches its SSH, as {machine} and {machine-port:22} give them. It runs
# in the test's own folder, which keeps what one step learned for the
# next.
set -eu

mode="$1"
lab_key="${XDG_CONFIG_HOME:-$HOME/.config}/dukovo-lab/lab_ed25519"
work=$(pwd)

# `release` needs neither the machine nor its port.
if [ "$mode" = release ]; then
	touch release
	left=120
	while [ "$left" -gt 0 ]; do
		if [ -f released ]; then
			echo "released"
			exit 0
		fi
		sleep 1
		left=$((left - 1))
	done
	echo "hold never answered"
	exit 1
fi

machine="$2"
port="$3"
hosts="$work/known_hosts"
options="-F /dev/null -p $port -o StrictHostKeyChecking=yes -o UserKnownHostsFile=$hosts -o LogLevel=ERROR -o ConnectTimeout=30"
key="-i $lab_key -o IdentitiesOnly=yes -o BatchMode=yes"

# The fingerprint of the Ed25519 host key that the machine shows at its
# port now, read with ssh-keyscan, which trusts nothing: tried for a
# minute and a half, since the port answers before sshd does.
scan() {
	tries=30
	while [ "$tries" -gt 0 ]; do
		ssh-keyscan -T 10 -t ed25519 -p "$port" "$machine" > scanned 2> /dev/null || true
		if [ -s scanned ]; then
			ssh-keygen -lf scanned | awk '{ print $2 }'
			return 0
		fi
		sleep 3
		tries=$((tries - 1))
	done
	echo "no host key: ssh-keyscan read none from the machine" >&2
	return 1
}

# Without the escape sequences and carriage returns a terminal's output
# has, which grep would trip on.
plain() {
	sed -e 's/\x1b\[[0-9;?]*[a-zA-Z]//g' -e 's/\r//g' "$1"
}

# Waits up to SECONDS until the file FILE holds PATTERN (a fixed string),
# or stops the script with what the file holds.
sees() {
	left="$3"
	while [ "$left" -gt 0 ]; do
		if plain "$1" | grep -q -F -- "$2"; then
			return 0
		fi
		sleep 1
		left=$((left - 1))
	done
	echo "$1 never said: $2"
	plain "$1" | tail -n 25
	exit 1
}

case "$mode" in
scan)
	fingerprint=$(scan)
	echo "$fingerprint" > installed.fp
	cp scanned known_hosts
	echo "host key: $fingerprint"
	;;

menu)
	# Rescue's sshd takes a minute after its screen says SSH is on, until
	# the card has its address: the sign-in is tried for as long.
	tries=30
	while [ "$tries" -gt 0 ]; do
		# shellcheck disable=SC2086 # the options are words
		TERM=dumb timeout 40 ssh $options $key -tt "lab@$machine" < /dev/null > menu.out 2>&1 || true
		if plain menu.out | grep -q -F 'Choose 0 to 13:'; then
			break
		fi
		sleep 3
		tries=$((tries - 1))
	done
	sees menu.out "Choose 0 to 13:" 1
	sees menu.out "lab signed in over SSH from " 1
	sees menu.out "What would you like to do?" 1
	echo "menu: signed in with the key"

	# The host key is the one the installed system had.
	now=$(scan)
	if [ "$now" = "$(cat installed.fp)" ]; then
		echo "host key: the installed system's"
	else
		echo "host key: Rescue shows $now, the installed system showed $(cat installed.fp)"
		exit 1
	fi

	# A password is refused: the installed system's sshd_config has
	# passwords off. The askpass program answers with a password, so that
	# ssh asks no one.
	printf '#!/bin/sh\necho not-the-password\n' > askpass
	chmod 700 askpass
	set +e
	# shellcheck disable=SC2086
	SSH_ASKPASS="$work/askpass" SSH_ASKPASS_REQUIRE=force ssh $options \
		-o PubkeyAuthentication=no -o PreferredAuthentications=password \
		-o NumberOfPasswordPrompts=1 "lab@$machine" true < /dev/null > password.out 2>&1
	status=$?
	set -e
	if [ "$status" -eq 255 ] && grep -q 'Permission denied' password.out; then
		echo "password: refused"
	else
		echo "password: not refused (status $status)"
		cat password.out
		exit 1
	fi

	# A command is refused: a session in Rescue is its menu alone.
	set +e
	# shellcheck disable=SC2086
	ssh $options $key "lab@$machine" 'echo ran-$((6 * 7))' < /dev/null > command.out 2>&1
	status=$?
	set -e
	if [ "$status" -ne 0 ] && ! grep -q 'ran-42' command.out; then
		echo "command: refused"
	else
		echo "command: not refused (status $status)"
		cat command.out
		exit 1
	fi
	;;

hold)
	trap 'kill $(cat ./*.pid 2> /dev/null) 2> /dev/null || true' EXIT
	rm -f hold.in hold.out release released
	mkfifo hold.in
	# shellcheck disable=SC2086
	TERM=dumb ssh $options $key -tt "lab@$machine" < hold.in > hold.out 2>&1 &
	echo $! > hold.ssh.pid
	# Held open, so that ssh does not see the end of its input.
	sleep 3600 > hold.in &
	echo $! > hold.holder.pid
	sees hold.out "Choose 0 to 13:" 60
	printf '6\r' > hold.in
	sees hold.out "Copy to which disk or partition" 60
	echo "hold ready"
	left=1200
	while [ ! -f release ]; do
		if [ "$left" -le 0 ]; then
			echo "never released"
			exit 1
		fi
		sleep 1
		left=$((left - 1))
	done
	# Answering with nothing ends the copy, and gives back the lock.
	printf '\r' > hold.in
	sees hold.out "name a disk other than the root's" 60
	kill "$(cat hold.ssh.pid)" "$(cat hold.holder.pid)" 2> /dev/null || true
	touch released
	# A step of the lab ends when the test does; this one has said all.
	exec sleep 3600
	;;

*)
	echo "no such mode: $mode"
	exit 1
	;;
esac
