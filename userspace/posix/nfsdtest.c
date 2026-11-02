// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the nfsd boot test runs (ADR 0341): Spark starts it as root with
// the services, authd(8) among them, the names capability and the
// network. It makes alice's account with user(8) and shares a folder of
// her home with share(1) to QEMU's network, to read and write and again
// only to read, and once more to no network; share(1) refuses a network
// that is not one. As alice, she cannot turn NFS on, nor have authd start
// a worker. `share nfs on` starts nfsd(8) on port 2049; the host's Linux
// client, which QEMU forwards a port of the host's loopback to, mounts
// the shares as root and works in them as alice's number. When the
// host's last file is there, the test checks what the host wrote, as
// alice's, and that root, served as no one, wrote nothing; then `share
// nfs off` stops nfsd and closes the port, and the shares are taken away
// again.

#include <arpa/inet.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <sys/stat.h>

#include "authd.h"
#include "check.h"
#include "names.h"
#include "programs.h"

// This program, beneath the root.
#define PROGRAM "system/tests/posix/nfsdtest"
#define USER "/system/commands/user"
#define SHARE "/system/commands/share"
#define SERVICE_FILE "/system/settings/spark/services/nfsd.toml"
#define ALICE 1000
// The folder shared, and the files the host writes there.
#define PUBLIC "/users/alice/Public"
#define FROM_HOST PUBLIC "/from-host.txt"
#define RENAMED PUBLIC "/renamed.txt"
#define MADE_BY_HOST PUBLIC "/made-by-host"
#define BIG PUBLIC "/made-by-host/big.bin"
#define LINK PUBLIC "/link"
// What root on the host tried to write, and the read-only share was
// asked to.
#define BY_ROOT PUBLIC "/by-root.txt"
#define REFUSED PUBLIC "/refused.txt"
// The host's last file, written when it has checked everything else.
#define DONE PUBLIC "/done.txt"
// How long the host may take, in seconds.
#define HOST_SECONDS 300
// The size of the file the host copies in, and back out.
#define BIG_SIZE (4 * 1024 * 1024)

// What a program wrote to its standard error.
static char errors[4096];

// Runs `argv` given `input`, as run_program does, with what it writes to
// its standard error in `errors`.
static int run_both(const char *const argv[], const char *input) {
	char path[64];
	snprintf(path, sizeof(path), "/tmp/nfsdtest-errors-%d", (int)getpid());
	int fd = open(path, O_RDWR | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
	int saved = dup(STDERR_FILENO);
	CHECK(fd >= 0 && saved >= 0);
	dup2(fd, STDERR_FILENO);
	int status = run_program(argv, input, strlen(input));
	dup2(saved, STDERR_FILENO);
	close(saved);
	ssize_t got = pread(fd, errors, sizeof(errors) - 1, 0);
	errors[got > 0 ? got : 0] = 0;
	close(fd);
	unlink(path);
	return status;
}

// Checks that `argv` given `input` exits with `status`, writes `out` and
// `err` among what it writes to its standard output and error.
static void says(const char *file, int line, int status, const char *input, const char *out,
		const char *err, const char *const argv[]) {
	int got = run_both(argv, input);
	if(got != status || !strstr(output, out) || !strstr(errors, err)) {
		char what[1024];
		snprintf(what, sizeof(what), "%s %s exited with %d, wrote \"%.300s\" and \"%.300s\"",
				argv[0], argv[1] ? argv[1] : "", got, output, errors);
		check_failed(file, line, what, 0);
	}
}

#define SAYS(status, input, out, err, ...) \
	says(__FILE__, __LINE__, status, input, out, err, (const char *const[]){__VA_ARGS__, NULL})

// The start of the file at `path`, or "" without it.
static const char *contents(const char *path) {
	static char data[4096];
	data[0] = 0;
	int fd = open(path, O_RDONLY | O_CLOEXEC);
	if(fd < 0)
		return data;
	ssize_t len = read(fd, data, sizeof(data) - 1);
	close(fd);
	data[len > 0 ? len : 0] = 0;
	return data;
}

// The file at `path`, with `text`, alice's.
static void write_text(const char *path, const char *text) {
	int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0644);
	CHECK(fd >= 0);
	if(fd < 0)
		return;
	CHECK(write(fd, text, strlen(text)) == (ssize_t)strlen(text));
	CHECK(fchown(fd, ALICE, ALICE) == 0);
	CHECK(close(fd) == 0);
}

// Whether something listens on port 2049 of the loopback.
static int listening(void) {
	int fd = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
	CHECK(fd >= 0);
	struct sockaddr_in at = {.sin_family = AF_INET, .sin_port = htons(2049)};
	at.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
	int connected = connect(fd, (struct sockaddr *)&at, sizeof(at)) == 0;
	close(fd);
	return connected;
}

// Waits up to `seconds` for port 2049 to be `open` or not; whether it is.
static int port_becomes(int open, int seconds) {
	long long deadline = check_now_ns() + seconds * 1000000000LL;
	while(listening() != open) {
		if(check_now_ns() > deadline)
			return 0;
		check_sleep_ms(200);
	}
	return 1;
}

// alice's account.
static void accounts(void) {
	SAYS(0, "wonderland\n", "Made alice, number 1000, home /users/alice\n", "", USER, "add",
			"alice");
}

// As alice: she shares nothing, cannot turn NFS on, and authd starts no
// worker for her.
static int alice(void) {
	check_start("nfsdtest (alice)");
	SAYS(1, "", "", "only root shares folders", SHARE, "add", "--nfs", "10.0.2.0/24", "Mine",
			"/users/alice");
	SAYS(1, "", "", "only root turns NFS on", SHARE, "nfs", "on");
	char answer[256];
	read_answer(send_request(REQUEST("nfs-worker\0" "1000\0")), answer, sizeof(answer));
	CHECK(strstr(answer, "no only nfsd") == answer);
	return check_end();
}

// The folder of alice's home that the shares are, and share(1) adding
// and listing them, and refusing a network that is not one.
static void sharing(void) {
	CHECK(mkdir(PUBLIC, 0755) == 0 && chown(PUBLIC, ALICE, ALICE) == 0);
	write_text(PUBLIC "/from-dukovo.txt", "written on Dukovo\n");
	SAYS(0, "", "NFS clients on 10.0.2.0/24 mount it as :/Public.", "", SHARE, "add", "--nfs",
			"10.0.2.0/24", "Public", PUBLIC);
	SAYS(0, "", "NFS is off; `share nfs on` turns it on.", "", SHARE, "add", "--read-only",
			"--nfs", "10.0.2.2", "Readonly", PUBLIC);
	// Shared to Windows PCs alone: no NFS client sees it.
	SAYS(0, "", "Shared /users/alice/Public as Private, read-write.", "", SHARE, "add", "Private",
			PUBLIC);
	SAYS(1, "", "", "10.0.2.0/33 is not an address", SHARE, "add", "--nfs", "10.0.2.0/33", "Bad", PUBLIC);
	SAYS(1, "", "", "nowhere is not an address", SHARE, "add", "--nfs", "nowhere", "Bad", PUBLIC);
	CONTAINS("NFS: 10.0.2.2", SHARE, "list");
	CONTAINS("NFS is off", SHARE, "list");
	CHECK(strstr(contents("/system/settings/shares.toml"), "nfs = [\"10.0.2.0/24\"]") != NULL);
}

// `share nfs on` starts nfsd, which listens on port 2049.
static void on(void) {
	CHECK(!listening());
	SAYS(0, "", "NFS is on", "", SHARE, "nfs", "on");
	CHECK(port_becomes(1, 30));
	CONTAINS("NFS is on", SHARE, "list");
	CHECK(strstr(contents(SERVICE_FILE), "\nenabled = true\n") != NULL);
}

// What the host's client did, which it ends by writing done.txt.
static void host(void) {
	printf("nfsdtest: ready for the host\n");
	fflush(stdout);
	struct stat st;
	long long deadline = check_now_ns() + HOST_SECONDS * 1000000000LL;
	while(stat(DONE, &st) != 0 && check_now_ns() < deadline)
		check_sleep_ms(500);
	CHECK(stat(DONE, &st) == 0 && st.st_uid == ALICE && st.st_gid == ALICE);
	// Files the host wrote, renamed and linked to, and a folder it made,
	// alice's.
	CHECK_ERROR(stat(FROM_HOST, &st), ENOENT);
	CHECK(!strcmp(contents(RENAMED), "written on the host\n"));
	CHECK(stat(RENAMED, &st) == 0 && st.st_uid == ALICE && st.st_gid == ALICE);
	CHECK((st.st_mode & 07777) == 0600);
	CHECK(stat(MADE_BY_HOST, &st) == 0 && S_ISDIR(st.st_mode) && st.st_uid == ALICE);
	CHECK(stat(BIG, &st) == 0 && st.st_size == BIG_SIZE && st.st_uid == ALICE);
	char target[64] = {0};
	CHECK(readlink(LINK, target, sizeof(target) - 1) > 0 && !strcmp(target, "renamed.txt"));
	// Root, served as no one, and the read-only share wrote nothing.
	CHECK_ERROR(stat(BY_ROOT, &st), ENOENT);
	CHECK_ERROR(stat(REFUSED, &st), ENOENT);
}

// `share nfs off` stops nfsd, which closes the port, and the shares are
// taken away.
static void off(void) {
	SAYS(0, "", "NFS is off", "", SHARE, "nfs", "off");
	CHECK(port_becomes(0, 30));
	CHECK(strstr(contents(SERVICE_FILE), "\nenabled = false\n") != NULL);
	SAYS(0, "", "Stopped sharing Readonly", "", SHARE, "remove", "readonly");
	SAYS(0, "", "Stopped sharing Public", "", SHARE, "remove", "Public");
	SAYS(0, "", "Stopped sharing Private", "", SHARE, "remove", "Private");
	CONTAINS("No folder is shared", SHARE, "list");
}

int main(int argc, char **argv) {
	if(argc == 2 && !strcmp(argv[1], "alice"))
		return alice();
	check_start("nfsdtest");
	authd_answers();
	RUN(accounts);
	// She notes the commands she runs, for the `commands` test, too.
	int notes = open(COMMANDS_RUN, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0644);
	CHECK(notes >= 0 && fchmod(notes, 0666) == 0);
	close(notes);
	char *const alice_args[] = {"nfsdtest", "alice", NULL};
	pid_t pid = start_as(PROGRAM, ALICE, ALICE, NULL, 0, alice_args);
	CHECK_CHILD(pid);
	RUN(sharing);
	RUN(on);
	RUN(host);
	RUN(off);
	return check_end();
}
