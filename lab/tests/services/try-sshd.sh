#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The machine's side of the try lab test (ADR 0472): tries, for SECONDS,
# a change to sshd's file that takes authd's socket from its view, so
# sshd runs but signs no one in, and the session that made the change is
# cut off as sshd restarts. Run in the background, its output in
# /tmp/lab-try-sshd.out.
#
#   try-sshd.sh SECONDS
set -eu

EDITOR='sed -i "s|^files.read = .*|files.read = []|"' \
	exec service try sshd --for "$1" < /dev/null
