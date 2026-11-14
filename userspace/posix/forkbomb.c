// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the limits boot test runs as a service with a thread limit
// (ADR 0476): it forks until a fork fails, says why and how many children
// it made, and keeps them until it is stopped. With `threads = 64` in its
// service file, the 64th thread of its domain, the first being its own,
// is not made, and fork fails with EAGAIN.

#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(void) {
	int made = 0;
	for(;;) {
		pid_t child = fork();
		if(child == 0) {
			// A child only stays: each counts as a thread of the domain.
			for(;;)
				pause();
		}
		if(child < 0) {
			printf("forkbomb: fork failed with %s after %d children\n",
				errno == EAGAIN ? "EAGAIN" : strerror(errno), made);
			fflush(stdout);
			break;
		}
		made++;
	}
	// The children stay while it does.
	for(;;)
		pause();
}
