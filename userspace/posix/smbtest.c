// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the smb boot test runs (ADR 0227): smb(1), mount(8) and umount(8)
// against the host's smbd, which QEMU's network reaches at 10.0.2.2, on
// the port, and as the user, that the host tells on 10.0.2.100 port 79.
// It lists the server's shares and a folder of one, and is refused a
// wrong password and a share that is not there; mounts the share with
// `mount -t cifs`, reads the host's file and writes one the host then
// checks; appends, cuts short, renames over another file, removes, and
// copies a 3 MiB file there and back; makes, moves and removes folders,
// a full one staying; marks a file read-only with chmod, which refuses
// writes until chmod takes it back, and makes a file read-only that is
// made without write bits; makes no links and changes no owner; mounts a
// folder of the share only to read, and unmounts it by its source; and
// mounts the case's FAT disk with mount(8), to write and only to read,
// and unmounts it by its name and by its device's path.

#include <arpa/inet.h>
#include <netinet/in.h>
#include <stdarg.h>
#include <sys/socket.h>
#include <sys/stat.h>

#include "programs.h"

// Where the host tells the port and the user.
#define ABOUT "10.0.2.100"

// The password the host gives its user's account in smbd.
#define PASSWORD "dukovo-smb-test"

// The credentials file, as mount.cifs reads one.
#define CREDENTIALS "/tmp/smb-credentials"

// Where the share and its folder are mounted.
#define SHARE "/tmp/share"
#define FOLDER "/tmp/folder"

// The case's FAT disk, which Spark mounts at /volumes/vda, and where
// mount(8) mounts it again.
#define DISK "/tmp/disk"

// //10.0.2.2:<port>, the share on it, and its folder.
static char server[64], share[96], folder[128];

// The share as the mounts list names it, without the port.
#define SOURCE "//10.0.2.2/share"

// A command for /bin/sh, made as printf makes text; two at a time.
static const char *sh(const char *format, ...) {
	static char command[2][512];
	static int turn;
	turn ^= 1;
	va_list args;
	va_start(args, format);
	vsnprintf(command[turn], sizeof(command[turn]), format, args);
	va_end(args);
	return command[turn];
}

// The port of the host's smbd and the user it signs in, from the host's
// port 79: "<port> <user>".
static void about_host(int *port, char *user, size_t size) {
	int fd = socket(AF_INET, SOCK_STREAM, 0);
	CHECK(fd >= 0);
	struct sockaddr_in at = {.sin_family = AF_INET, .sin_port = htons(79)};
	CHECK(inet_pton(AF_INET, ABOUT, &at.sin_addr) == 1);
	CHECK(connect(fd, (struct sockaddr *)&at, sizeof(at)) == 0);
	char line[128];
	size_t len = 0;
	ssize_t got;
	while(len < sizeof(line) - 1 && (got = read(fd, line + len, sizeof(line) - 1 - len)) > 0)
		len += got;
	close(fd);
	line[len] = 0;
	char name[64] = "";
	CHECK(sscanf(line, "%d %63s", port, name) == 2);
	snprintf(user, size, "%s", name);
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

// The server's shares, and a folder of one; a wrong password and a share
// that is not there are refused.
static void listing(void) {
	CONTAINS("share  Dukovo's test share\n", "/bin/sh", "-c",
			sh("smb shares -o credentials=" CREDENTIALS " %s", server));
	OUTPUT("", "folder/\nfrom-host.txt\n", "/bin/sh", "-c",
			sh("smb ls -o credentials=" CREDENTIALS " %s", share));
	OUTPUT("", "inner.txt\n", "/bin/sh", "-c",
			sh("smb ls -o credentials=" CREDENTIALS " %s", folder));
	STATUS(1, "/bin/sh", "-c",
			sh("smb ls -o credentials=" CREDENTIALS ",password=wrong %s 2>/dev/null", share));
	STATUS(1, "/bin/sh", "-c", sh("smb ls -o credentials=" CREDENTIALS " %s/nothing 2>/dev/null", server));
	STATUS(2, "/bin/sh", "-c", "smb ls 2>/dev/null");
}

// The share mounted with mount -t cifs, which runs smb: listed as a
// share's mount, the host's file read, its modes as asked.
static void mounted(void) {
	STATUS(0, "/bin/mkdir", "-p", SHARE);
	STATUS(0, "/bin/sh", "-c",
			sh("mount -t cifs -o credentials=" CREDENTIALS ",uid=0,gid=0,file_mode=0644 %s " SHARE,
					share));
	CONTAINS(SOURCE " on " SHARE " type fuse (rw)\n", "/bin/mount", "-t", "smb");
	OUTPUT("", "folder\nfrom-host.txt\n", "/bin/ls", SHARE);
	OUTPUT("", "from the host\n", "/bin/cat", SHARE "/from-host.txt");
	OUTPUT("", "-rw-r--r-- 14\n", "/bin/sh", "-c",
			"ls -l " SHARE "/from-host.txt | awk '{print $1, $5}'");
	OUTPUT("", "drwxr-xr-x\n", "/bin/sh", "-c", "ls -ld " SHARE "/folder | awk '{print $1}'");
}

// Files written, appended to, cut short, renamed over another, removed,
// and a large one copied there and back.
static void files(void) {
	STATUS(0, "/bin/sh", "-c", "echo from dukovo > " SHARE "/from-guest.txt");
	OUTPUT("", "from dukovo\n", "/bin/cat", SHARE "/from-guest.txt");
	STATUS(0, "/bin/sh", "-c", "echo more >> " SHARE "/scratch.txt && echo more >> " SHARE "/scratch.txt");
	OUTPUT("", "more\nmore\n", "/bin/cat", SHARE "/scratch.txt");
	STATUS(0, "/bin/sh", "-c", ": > " SHARE "/scratch.txt");
	OUTPUT("", "0\n", "/bin/sh", "-c", "wc -c < " SHARE "/scratch.txt");
	STATUS(0, "/bin/truncate", "-s", "10", SHARE "/scratch.txt");
	OUTPUT("", "10\n", "/bin/sh", "-c", "wc -c < " SHARE "/scratch.txt");
	STATUS(0, "/bin/sh", "-c", "echo replacing > " SHARE "/new.txt");
	STATUS(0, "/bin/mv", SHARE "/new.txt", SHARE "/scratch.txt");
	OUTPUT("", "replacing\n", "/bin/cat", SHARE "/scratch.txt");
	STATUS(1, "/bin/sh", "-c", "cat " SHARE "/new.txt 2>/dev/null");
	STATUS(0, "/bin/rm", SHARE "/scratch.txt");
	STATUS(1, "/bin/sh", "-c", "cat " SHARE "/scratch.txt 2>/dev/null");
	// 3 MiB, in many of the 64 KiB a READ or WRITE carries.
	FILE *big = fopen("/tmp/big", "w");
	CHECK(big != NULL);
	if(big) {
		for(unsigned i = 0; i < 3 * 1024 * 1024 / 4; i++) {
			unsigned word = i * 2654435761u;
			fwrite(&word, sizeof(word), 1, big);
		}
		CHECK(fclose(big) == 0);
	}
	STATUS(0, "/bin/cp", "/tmp/big", SHARE "/big");
	STATUS(0, "/bin/cmp", "/tmp/big", SHARE "/big");
	STATUS(0, "/bin/cp", SHARE "/big", "/tmp/big-back");
	STATUS(0, "/bin/cmp", "/tmp/big", "/tmp/big-back");
	OUTPUT("", "3145728\n", "/bin/sh", "-c", "wc -c < " SHARE "/big");
	STATUS(0, "/bin/rm", SHARE "/big", "/tmp/big-back");
}

// Folders made, filled, moved with what is in them, and removed; a full
// one stays.
static void folders(void) {
	STATUS(0, "/bin/mkdir", "-p", SHARE "/made/deeper");
	write_text(SHARE "/made/deeper/a", "a\n");
	STATUS(1, "/bin/sh", "-c", "rmdir " SHARE "/made/deeper 2>/dev/null");
	OUTPUT("", "a\n", "/bin/ls", SHARE "/made/deeper");
	STATUS(0, "/bin/mv", SHARE "/made", SHARE "/moved");
	OUTPUT("", "a\n", "/bin/cat", SHARE "/moved/deeper/a");
	STATUS(1, "/bin/sh", "-c", "test -e " SHARE "/made");
	STATUS(0, "/bin/rm", SHARE "/moved/deeper/a");
	STATUS(0, "/bin/rmdir", SHARE "/moved/deeper", SHARE "/moved");
	STATUS(0, "/bin/mv", SHARE "/folder", SHARE "/renamed");
	OUTPUT("", "inside\n", "/bin/cat", SHARE "/renamed/inner.txt");
	STATUS(0, "/bin/mv", SHARE "/renamed", SHARE "/folder");
	OUTPUT("", "folder\nfrom-guest.txt\nfrom-host.txt\n", "/bin/ls", SHARE);
}

// chmod marks a file read-only, which refuses writes, and takes it back;
// owners do not change, and links are not made.
static void modes(void) {
	STATUS(0, "/bin/chmod", "a-w", SHARE "/from-guest.txt");
	OUTPUT("", "-r--r--r--\n", "/bin/sh", "-c", "ls -l " SHARE "/from-guest.txt | awk '{print $1}'");
	STATUS(3, "/bin/sh", "-c",
			"if echo no >> " SHARE "/from-guest.txt; then exit 0; else exit 3; fi 2>/dev/null");
	STATUS(0, "/bin/chmod", "u+w", SHARE "/from-guest.txt");
	OUTPUT("", "-rw-r--r--\n", "/bin/sh", "-c", "ls -l " SHARE "/from-guest.txt | awk '{print $1}'");
	OUTPUT("", "from dukovo\n", "/bin/cat", SHARE "/from-guest.txt");
	STATUS(1, "/bin/sh", "-c", "chown 1000 " SHARE "/from-guest.txt 2>/dev/null");
	STATUS(1, "/bin/sh", "-c", "ln -s from-host.txt " SHARE "/link 2>/dev/null");
	STATUS(1, "/bin/sh", "-c", "ln " SHARE "/from-host.txt " SHARE "/hard 2>/dev/null");
	// Made without write bits, as install -m 444 makes a file: written
	// through the descriptor that made it, read-only from then on.
	int fd = open(SHARE "/made-ro.txt", O_WRONLY | O_CREAT | O_EXCL, 0444);
	CHECK(fd >= 0);
	if(fd >= 0) {
		CHECK(write(fd, "kept\n", 5) == 5);
		CHECK(close(fd) == 0);
	}
	OUTPUT("", "-r--r--r-- 5\n", "/bin/sh", "-c",
			"ls -l " SHARE "/made-ro.txt | awk '{print $1, $5}'");
	int again = open(SHARE "/made-ro.txt", O_WRONLY);
	CHECK(again < 0 && errno == EACCES);
	if(again >= 0)
		close(again);
	STATUS(0, "/bin/chmod", "u+w", SHARE "/made-ro.txt");
	STATUS(0, "/bin/rm", SHARE "/made-ro.txt");
}

// A folder of the share, mounted only to read with smb itself, and
// unmounted by its source; the share is unmounted by its folder.
static void unmounting(void) {
	STATUS(0, "/bin/mkdir", "-p", FOLDER);
	STATUS(0, "/bin/sh", "-c",
			sh("smb mount -o credentials=" CREDENTIALS ",ro %s " FOLDER, folder));
	CONTAINS(SOURCE "/folder on " FOLDER " type fuse (ro)\n", "/bin/mount");
	OUTPUT("", "inner.txt\n", "/bin/ls", FOLDER);
	STATUS(3, "/bin/sh", "-c", "if echo no > " FOLDER "/new; then exit 0; else exit 3; fi 2>/dev/null");
	STATUS(0, "/bin/umount", SOURCE "/folder");
	STATUS(0, "/bin/umount", SHARE);
	OUTPUT("", "", "/bin/ls", SHARE);
	OUTPUT("", "", "/bin/ls", FOLDER);
	OUTPUT("", "", "/bin/mount", "-t", "cifs");
	STATUS(1, "/bin/sh", "-c", "umount " SHARE " 2>/dev/null");
}

// The case's FAT disk, unmounted from /volumes/vda and mounted with
// mount(8): its kind found, to write; then as fat, only to read.
static void disk(void) {
	CONTAINS("vda on /volumes/vda type fat (rw)", "/bin/mount", "-t", "fat");
	STATUS(0, "/bin/umount", "/volumes/vda");
	STATUS(0, "/bin/mkdir", "-p", DISK);
	STATUS(0, "/bin/mount", "vda", DISK);
	STATUS(0, "/bin/sh", "-c", "echo on the disk > " DISK "/mounted.txt");
	STATUS(0, "/bin/umount", "vda");
	STATUS(0, "/bin/mount", "-r", "-t", "fat", "/devices/vda", DISK);
	CONTAINS(" on " DISK " type fat (ro)", "/bin/mount");
	OUTPUT("", "on the disk\n", "/bin/cat", DISK "/mounted.txt");
	STATUS(3, "/bin/sh", "-c", "if echo no > " DISK "/new; then exit 0; else exit 3; fi 2>/dev/null");
	STATUS(1, "/bin/sh", "-c", "mount -t ext4 vda " FOLDER " 2>/dev/null");
	STATUS(0, "/bin/umount", "/devices/vda");
	OUTPUT("", "", "/bin/mount", "-t", "fat");
}

int main(void) {
	check_start("smbtest");
	int port = 0;
	char user[64] = "";
	about_host(&port, user, sizeof(user));
	snprintf(server, sizeof(server), "//10.0.2.2:%d", port);
	snprintf(share, sizeof(share), "%s/share", server);
	snprintf(folder, sizeof(folder), "%s/folder", share);
	char credentials[160];
	snprintf(credentials, sizeof(credentials),
			"# The host's account in its smbd\nusername=%s\npassword=" PASSWORD "\n", user);
	write_text(CREDENTIALS, credentials);
	CHECK(chmod(CREDENTIALS, 0600) == 0);
	RUN(listing);
	RUN(mounted);
	RUN(files);
	RUN(folders);
	RUN(modes);
	RUN(unmounting);
	RUN(disk);
	return check_end();
}
