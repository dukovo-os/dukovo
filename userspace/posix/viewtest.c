// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the service-views boot test runs (ADR 0473): a service that holds
// some of the root's subvolumes, on a Dukovo root of Btrfs. viewtest
// writes a service file with `files.root-subvolumes = ["@snapshots"]` and
// starts it, as `viewtest --inside`, in the view Spark builds of it. That
// view lists /volumes/DISK with `@snapshots` alone; takes no new name
// there, and no name of another subvolume, whether made, removed or
// renamed into; and takes a snapshot of /system/settings into the
// subvolume it holds, as the snap service makes its own. `service status`
// says what it holds, and `service check` refuses a file that names a
// subvolume of the owner's.

#include <dirent.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

#include "check.h"
#include "programs.h"

#define SERVICE "/system/commands/service"
#define SUBVOLUME "/system/commands/subvolume"
#define SERVICES "/system/settings/spark/services"
#define SELF "/system/tests/posix/viewtest"

// Its own folder, where it says how its checks went.
#define OWN "/system/state/viewtest"

// The service that holds @snapshots, and writes in its own folder.
static const char HOLDER[] = "service_version = 2\n"
		"description = \"Holds @snapshots at the top of the root\"\n"
		"program = \"" SELF "\"\n"
		"arguments = [\"--inside\"]\n"
		"enabled = false\n"
		"restart = \"never\"\n"
		"\n"
		"[permissions]\n"
		"files.root-subvolumes = [\"@snapshots\"]\n"
		"files.read-write = [\"" OWN "\"]\n";

// A file that names the subvolume of the owner's files.
static const char GREEDY[] = "service_version = 2\n"
		"description = \"Asks for the owner's files\"\n"
		"program = \"" SELF "\"\n"
		"enabled = false\n"
		"\n"
		"[permissions]\n"
		"files.root-subvolumes = [\"@snapshots\", \"@users\"]\n";

// Writes `text` to `path`.
static void write_file(const char *path, const char *text) {
	int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0644);
	CHECK(fd >= 0);
	if(fd < 0)
		return;
	CHECK(write(fd, text, strlen(text)) == (ssize_t)strlen(text));
	close(fd);
}

// Writes `text` to `path` so that it is there whole or not at all: it is
// written beside the file and renamed into place, as held() reads the
// file the moment it is there.
static void put_file(const char *path, const char *text) {
	char partial[128];
	snprintf(partial, sizeof(partial), "%s.partial", path);
	write_file(partial, text);
	CHECK(rename(partial, path) == 0);
}

// The text of `path`, in `buffer` of `size` bytes, NUL-terminated; empty
// if it cannot be read.
static const char *read_file(const char *path, char *buffer, size_t size) {
	buffer[0] = 0;
	int fd = open(path, O_RDONLY | O_CLOEXEC);
	if(fd < 0)
		return buffer;
	ssize_t got = read(fd, buffer, size - 1);
	buffer[got > 0 ? got : 0] = 0;
	close(fd);
	return buffer;
}

// Whether `path` is there.
static int exists(const char *path) {
	struct stat st;
	return stat(path, &st) == 0;
}

// Waits up to `seconds` for `path` to be there.
static int appears(const char *path, int seconds) {
	long long deadline = check_now_ns() + seconds * 1000000000LL;
	while(!exists(path)) {
		if(check_now_ns() >= deadline)
			return 0;
		check_sleep_ms(200);
	}
	return 1;
}

// The names in the folder `path`, but `.` and `..`, in `names` of `size`
// bytes, a space after each; how many there are, or -1 if it cannot be
// read.
static int names_in(const char *path, char *names, size_t size) {
	names[0] = 0;
	DIR *folder = opendir(path);
	if(!folder)
		return -1;
	int count = 0;
	struct dirent *entry;
	while((entry = readdir(folder))) {
		if(!strcmp(entry->d_name, ".") || !strcmp(entry->d_name, ".."))
			continue;
		size_t used = strlen(names);
		if(used + strlen(entry->d_name) + 2 > size) {
			closedir(folder);
			return -1;
		}
		snprintf(names + used, size - used, "%s ", entry->d_name);
		count++;
	}
	closedir(folder);
	return count;
}

// What the holder checks from inside its view.
static void inside(void) {
	// The root's Btrfs is at /volumes/DISK, where the disk is the only
	// name, and only @snapshots is in it.
	char names[256];
	CHECK(names_in("/volumes", names, sizeof(names)) == 1);
	char top[300];
	snprintf(top, sizeof(top), "/volumes/%.*s", (int)strcspn(names, " "), names);
	CHECK(strcmp(top, "/volumes/") != 0);
	CHECK(names_in(top, names, sizeof(names)) == 1);
	CHECK(!strcmp(names, "@snapshots "));
	printf("inside: %s holds %s\n", top, names);
	// Nothing else of the root is there to open.
	char path[400];
	const char *others[] = {"@users", "@shared", "@applications", "@settings", "@state"};
	for(size_t i = 0; i < sizeof(others) / sizeof(others[0]); i++) {
		snprintf(path, sizeof(path), "%s/%s", top, others[i]);
		CHECK(!exists(path));
	}

	// A name it does not hold cannot be made, and one it never saw
	// cannot be removed.
	snprintf(path, sizeof(path), "%s/@x", top);
	CHECK_ERROR(mkdir(path, 0755), EPERM);
	CHECK(!exists(path));
	snprintf(path, sizeof(path), "%s/@users", top);
	CHECK_ERROR(rmdir(path), ENOENT);

	// A folder it holds can be renamed within, and no name outside what
	// it holds can be renamed to.
	char from[400];
	snprintf(from, sizeof(from), "%s/@snapshots/viewtest-a", top);
	CHECK(mkdir(from, 0755) == 0);
	CHECK_ERROR(rename(from, path), EPERM);
	char to[400];
	snprintf(to, sizeof(to), "%s/@snapshots/viewtest-b", top);
	CHECK(rename(from, to) == 0);
	CHECK(rmdir(to) == 0);

	// A snapshot of /system/settings, which is read-only here, into what it
	// holds, as the snap service makes its own.
	char folder[400];
	char snapshot[420];
	snprintf(folder, sizeof(folder), "%s/@snapshots/viewtest", top);
	snprintf(snapshot, sizeof(snapshot), "%s/t", folder);
	CHECK(mkdir(folder, 0755) == 0);
	STATUS(0, SUBVOLUME, "snapshot", "-r", "/system/settings", snapshot);
	char passwd[460];
	snprintf(passwd, sizeof(passwd), "%s/passwd", snapshot);
	CHECK(exists(passwd));
	STATUS(0, SUBVOLUME, "delete", snapshot);
	CHECK(rmdir(folder) == 0);
}

// The service that holds @snapshots checks its view, and `service status`
// says what it holds.
static void held(void) {
	char text[64];
	CHECK(exists("/system/live/spark/root"));
	write_file(SERVICES "/view-holder.toml", HOLDER);
	CHECK(mkdir(OWN, 0755) == 0);
	STATUS(0, SERVICE, "check", "view-holder");
	STATUS(0, SERVICE, "view-holder", "start");
	CHECK(appears(OWN "/done", 30));
	CHECK(!strcmp(read_file(OWN "/done", text, sizeof(text)), "every check passed"));
	// It has ended, so status's exit status says nothing: only its text does.
	run_program((const char *const[]){SERVICE, "view-holder", "status", NULL}, "", 0);
	if(!strstr(output, "changes @snapshots at the top of the root, and nothing else there"))
		printf("viewtest: view-holder's status said:\n%s", output);
	CHECK(strstr(output, "changes @snapshots at the top of the root, and nothing else there"));
}

// A file that names a subvolume of the owner's is refused, with the name.
static void refused(void) {
	output_errors = 1;
	write_file(SERVICES "/view-greedy.toml", GREEDY);
	RUN_WITH(1, "", "files.root-subvolumes cannot name @users", 1, SERVICE, "check", "view-greedy");
	output_errors = 0;
	CHECK(unlink(SERVICES "/view-greedy.toml") == 0);
}

int main(int argc, char **argv) {
	if(argc == 2 && !strcmp(argv[1], "--inside")) {
		check_start("inside");
		inside();
		int status = check_end();
		put_file(OWN "/done", status ? "failed" : "every check passed");
		return status;
	}
	check_start("viewtest");
	RUN(held);
	RUN(refused);
	return check_end();
}
