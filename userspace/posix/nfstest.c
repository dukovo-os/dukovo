// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the nfs boot test runs (ADR 0344): nfs(1), mount(8) and umount(8)
// against the host's own Linux NFS server, which QEMU's network reaches
// at 10.0.2.2, exporting the folders the host names on 10.0.2.100 port
// 79: one to read and write, owned by number 1000, and one only to read.
// It refuses command lines nfs cannot take; lists a folder and refuses
// one that is not there; mounts the first with `mount -t nfs`, where
// root, whom the server serves as no one, may not write; then, as number
// 1000, writes, appends to, cuts short, renames over another, removes
// and copies a 4 MiB file there and back, makes, moves and removes
// folders, links and changes modes, and finds the space left; unmounts
// it, and mounts the second without -t, which refuses writes, and
// unmounts it by its source. Where the host has no NFS server to give,
// only the command lines are checked.

#include <arpa/inet.h>
#include <netinet/in.h>
#include <stdarg.h>
#include <sys/socket.h>
#include <sys/stat.h>

#include "names.h"
#include "programs.h"

// Where the host names the folders.
#define ABOUT "10.0.2.100"

// Where the folders are mounted.
#define MOUNTED "/tmp/nfs"
#define READ_ONLY "/tmp/nfs-ro"

// The number the host's files are owned by, which writes as itself.
#define ALICE 1000

// This program, which runs again as number 1000.
#define PROGRAM "system/tests/posix/nfstest"

// The large file's size, which the host checks too.
#define BIG (4 * 1024 * 1024)

// 10.0.2.2:<folder>, each.
static char source[320], read_only[320];

// A command for /bin/sh, made as printf makes text; two at a time.
static const char *sh(const char *format, ...) {
	static char command[2][768];
	static int turn;
	turn ^= 1;
	va_list args;
	va_start(args, format);
	vsnprintf(command[turn], sizeof(command[turn]), format, args);
	va_end(args);
	return command[turn];
}

// The folders the host exports, from its port 79: "ready <folder>
// <read-only folder>", or "none" where it has no NFS server to give.
static int about_host(void) {
	int fd = socket(AF_INET, SOCK_STREAM, 0);
	CHECK(fd >= 0);
	struct sockaddr_in at = {.sin_family = AF_INET, .sin_port = htons(79)};
	CHECK(inet_pton(AF_INET, ABOUT, &at.sin_addr) == 1);
	CHECK(connect(fd, (struct sockaddr *)&at, sizeof(at)) == 0);
	char line[600];
	size_t len = 0;
	ssize_t got;
	while(len < sizeof(line) - 1 && (got = read(fd, line + len, sizeof(line) - 1 - len)) > 0)
		len += got;
	close(fd);
	line[len] = 0;
	char folder[300] = "", ro[300] = "";
	if(sscanf(line, "ready %299s %299s", folder, ro) != 2) {
		CHECK(strncmp(line, "none", 4) == 0);
		return 0;
	}
	snprintf(source, sizeof(source), "10.0.2.2:%s", folder);
	snprintf(read_only, sizeof(read_only), "10.0.2.2:%s", ro);
	return 1;
}

// The file at `path`, with `text`.
static void write_text(const char *path, const char *text) {
	FILE *file = fopen(path, "w");
	CHECK(file != NULL);
	if(!file)
		return;
	fputs(text, file);
	CHECK(fclose(file) == 0);
}

// What nfs refuses before it reaches a server.
static void command_lines(void) {
	STATUS(2, "/bin/sh", "-c", "nfs 2>/dev/null");
	STATUS(2, "/bin/sh", "-c", "nfs ls -o vers=3 10.0.2.2:/srv 2>/dev/null");
	STATUS(2, "/bin/sh", "-c", "nfs ls -o sec=lkey 10.0.2.2:/srv 2>/dev/null");
	STATUS(2, "/bin/sh", "-c", "nfs ls nas 2>/dev/null");
	STATUS(2, "/bin/sh", "-c", "nfs mount 10.0.2.2:/srv 2>/dev/null");
	CONTAINS("usage: nfs mount", "/bin/sh", "-c", "nfs --help");
}

// A folder listed, and one that is not there refused.
static void listing(void) {
	OUTPUT("", "folder/\nfrom-host.txt\n", "/bin/sh", "-c", sh("nfs ls %s", source));
	OUTPUT("", "inner.txt\n", "/bin/sh", "-c", sh("nfs ls %s/folder", source));
	STATUS(1, "/bin/sh", "-c", sh("nfs ls %s/nothing 2>/dev/null", source));
}

// Mounted with mount -t nfs, which runs nfs: listed as an NFS folder's
// mount, the host's file read, with its owner and mode; root may not
// write there.
static void mounted(void) {
	STATUS(0, "/bin/mkdir", "-p", MOUNTED);
	STATUS(0, "/bin/sh", "-c", sh("mount -t nfs -o vers=4.2,soft,timeo=100 %s " MOUNTED, source));
	CONTAINS(sh("%s on " MOUNTED " type fuse (rw)\n", source), "/bin/mount", "-t", "nfs");
	OUTPUT("", "folder\nfrom-host.txt\n", "/bin/ls", MOUNTED);
	OUTPUT("", "from the host\n", "/bin/cat", MOUNTED "/from-host.txt");
	OUTPUT("", "-rw-r--r-- 1000 1000 14\n", "/bin/sh", "-c",
			"ls -ln " MOUNTED "/from-host.txt | awk '{print $1, $3, $4, $5}'");
	STATUS(3, "/bin/sh", "-c",
			"if echo no > " MOUNTED "/by-root.txt; then exit 0; else exit 3; fi 2>/dev/null");
}

// Files written, appended to, cut short, renamed over another, removed,
// and a large one copied there and back, which stays for the host.
static void files(void) {
	STATUS(0, "/bin/sh", "-c", "echo from dukovo > " MOUNTED "/from-guest.txt");
	OUTPUT("", "from dukovo\n", "/bin/cat", MOUNTED "/from-guest.txt");
	OUTPUT("", "1000 1000\n", "/bin/sh", "-c",
			"ls -ln " MOUNTED "/from-guest.txt | awk '{print $3, $4}'");
	STATUS(0, "/bin/sh", "-c",
			"echo more >> " MOUNTED "/scratch.txt && echo more >> " MOUNTED "/scratch.txt");
	OUTPUT("", "more\nmore\n", "/bin/cat", MOUNTED "/scratch.txt");
	STATUS(0, "/bin/sh", "-c", ": > " MOUNTED "/scratch.txt");
	OUTPUT("", "0\n", "/bin/sh", "-c", "wc -c < " MOUNTED "/scratch.txt");
	STATUS(0, "/bin/truncate", "-s", "10", MOUNTED "/scratch.txt");
	OUTPUT("", "10\n", "/bin/sh", "-c", "wc -c < " MOUNTED "/scratch.txt");
	STATUS(0, "/bin/sh", "-c", "echo replacing > " MOUNTED "/new.txt");
	STATUS(0, "/bin/mv", MOUNTED "/new.txt", MOUNTED "/scratch.txt");
	OUTPUT("", "replacing\n", "/bin/cat", MOUNTED "/scratch.txt");
	STATUS(1, "/bin/sh", "-c", "cat " MOUNTED "/new.txt 2>/dev/null");
	STATUS(0, "/bin/rm", MOUNTED "/scratch.txt");
	STATUS(1, "/bin/sh", "-c", "cat " MOUNTED "/scratch.txt 2>/dev/null");
	// 4 MiB, words the host makes too, in several of the 1 MiB a WRITE
	// carries.
	FILE *big = fopen("/tmp/big", "w");
	CHECK(big != NULL);
	if(big) {
		for(unsigned i = 0; i < BIG / 4; i++) {
			unsigned word = i * 2654435761u;
			fwrite(&word, sizeof(word), 1, big);
		}
		CHECK(fclose(big) == 0);
	}
	STATUS(0, "/bin/cp", "/tmp/big", MOUNTED "/big");
	STATUS(0, "/bin/cmp", "/tmp/big", MOUNTED "/big");
	STATUS(0, "/bin/cp", MOUNTED "/big", "/tmp/big-back");
	STATUS(0, "/bin/cmp", "/tmp/big", "/tmp/big-back");
	OUTPUT("", "4194304\n", "/bin/sh", "-c", "wc -c < " MOUNTED "/big");
	STATUS(0, "/bin/rm", "/tmp/big", "/tmp/big-back");
}

// Folders made, filled, moved with what is in them, and removed; a full
// one stays.
static void folders(void) {
	STATUS(0, "/bin/mkdir", "-p", MOUNTED "/made/deeper");
	write_text(MOUNTED "/made/deeper/a", "a\n");
	STATUS(1, "/bin/sh", "-c", "rmdir " MOUNTED "/made/deeper 2>/dev/null");
	OUTPUT("", "a\n", "/bin/ls", MOUNTED "/made/deeper");
	STATUS(0, "/bin/mv", MOUNTED "/made", MOUNTED "/moved");
	OUTPUT("", "a\n", "/bin/cat", MOUNTED "/moved/deeper/a");
	STATUS(1, "/bin/sh", "-c", "test -e " MOUNTED "/made");
	STATUS(0, "/bin/rm", MOUNTED "/moved/deeper/a");
	STATUS(0, "/bin/rmdir", MOUNTED "/moved/deeper", MOUNTED "/moved");
	OUTPUT("", "big\nfolder\nfrom-guest.txt\nfrom-host.txt\n", "/bin/ls", MOUNTED);
}

// Symbolic and hard links made and followed; modes changed, and owners
// not, by one who is not root; the space left found.
static void links_and_modes(void) {
	STATUS(0, "/bin/ln", "-s", "from-guest.txt", MOUNTED "/link");
	OUTPUT("", "from-guest.txt\n", "/bin/readlink", MOUNTED "/link");
	OUTPUT("", "from dukovo\n", "/bin/cat", MOUNTED "/link");
	STATUS(0, "/bin/ln", MOUNTED "/from-guest.txt", MOUNTED "/hard");
	OUTPUT("", "2\n", "/bin/sh", "-c", "ls -ln " MOUNTED "/hard | awk '{print $2}'");
	STATUS(0, "/bin/rm", MOUNTED "/hard");
	STATUS(0, "/bin/chmod", "600", MOUNTED "/from-guest.txt");
	OUTPUT("", "-rw-------\n", "/bin/sh", "-c",
			"ls -l " MOUNTED "/from-guest.txt | awk '{print $1}'");
	STATUS(0, "/bin/chmod", "644", MOUNTED "/from-guest.txt");
	STATUS(1, "/bin/sh", "-c", "chown 1001 " MOUNTED "/from-guest.txt 2>/dev/null");
	// Made without write bits: written through the descriptor that made
	// it, refused after.
	int fd = open(MOUNTED "/made-ro.txt", O_WRONLY | O_CREAT | O_EXCL, 0444);
	CHECK(fd >= 0);
	if(fd >= 0) {
		CHECK(write(fd, "kept\n", 5) == 5);
		CHECK(fsync(fd) == 0);
		CHECK(close(fd) == 0);
	}
	OUTPUT("", "-r--r--r-- 5\n", "/bin/sh", "-c",
			"ls -l " MOUNTED "/made-ro.txt | awk '{print $1, $5}'");
	int again = open(MOUNTED "/made-ro.txt", O_WRONLY);
	CHECK(again < 0 && errno == EACCES);
	if(again >= 0)
		close(again);
	STATUS(0, "/bin/rm", MOUNTED "/made-ro.txt");
	STATUS(0, "/bin/sh", "-c", "df " MOUNTED " >/dev/null");
}

// As number 1000, started again as authd starts a person's programs
// (`--names`): the files, folders, links and modes.
static int alice(void) {
	check_start("nfstest");
	CHECK(getuid() == ALICE && getgid() == ALICE);
	RUN(files);
	RUN(folders);
	RUN(links_and_modes);
	return check_end();
}

// Unmounted by its folder; the read-only folder mounted without -t,
// read, refusing writes, and unmounted by its source.
static void unmounting(void) {
	STATUS(0, "/bin/umount", MOUNTED);
	OUTPUT("", "", "/bin/ls", MOUNTED);
	OUTPUT("", "", "/bin/mount", "-t", "nfs");
	STATUS(0, "/bin/mkdir", "-p", READ_ONLY);
	STATUS(0, "/bin/sh", "-c", sh("mount -o soft,timeo=100 %s " READ_ONLY, read_only));
	CONTAINS(sh("%s on " READ_ONLY " type fuse (rw)\n", read_only), "/bin/mount", "-t", "nfs4");
	OUTPUT("", "only to read\n", "/bin/cat", READ_ONLY "/ro.txt");
	OUTPUT("", "Read-only file system\n", "/bin/sh", "-c",
			"echo no 2>&1 > " READ_ONLY "/new.txt | sed 's/.*: //'");
	STATUS(0, "/bin/umount", read_only);
	OUTPUT("", "", "/bin/mount", "-t", "nfs");
	STATUS(1, "/bin/sh", "-c", "umount " MOUNTED " 2>/dev/null");
}

int main(int argc, char **argv) {
	if(argc == 2 && !strcmp(argv[1], "alice"))
		return alice();
	check_start("nfstest");
	int server = about_host();
	RUN(command_lines);
	if(!server) {
		printf("nfstest: the host gives no NFS server: the rest is left out\n");
		return check_end();
	}
	RUN(listing);
	RUN(mounted);
	// Number 1000 notes the commands it runs, for the `commands` test,
	// too.
	int notes = open(COMMANDS_RUN, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0644);
	CHECK(notes >= 0 && fchmod(notes, 0666) == 0);
	close(notes);
	char *const alice_args[] = {"nfstest", "alice", NULL};
	const uint32_t groups[] = {ALICE};
	pid_t pid = start_as(PROGRAM, ALICE, ALICE, groups, 1, alice_args);
	CHECK_CHILD(pid);
	RUN(unmounting);
	return check_end();
}
