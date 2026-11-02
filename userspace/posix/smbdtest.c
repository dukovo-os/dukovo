// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the smbd boot test runs (ADR 0228): Spark starts it as root with
// the services, authd(8) among them, the names capability and the
// network. It makes alice's account with user(8), sets her sharing
// password with share(1) as root, and then, as alice, she changes it
// given her login password and not without it, shares nothing and reads
// no sharing password. It shares a folder of her home twice, to read and
// write and only to read, and share(1) refuses a name that is taken or
// cannot be a share's, a folder that is not there, and a share that is
// not there to remove. `share on` starts smbd(8) on port 445; the host's
// smbclient, which QEMU forwards a port of the host's loopback to it,
// lists the shares, copies files both ways, makes a folder, and is
// refused a wrong password, a write to the read-only share and SMB 1.
// When the host's last file is there, the test checks what the host
// wrote, as alice's, and the sign-ins last(1) and lastb(1) show; then
// `share off` stops smbd and closes the port, and the shares and the
// sharing password are taken away again.

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
#define PROGRAM "system/tests/posix/smbdtest"
#define USER "/system/commands/user"
#define SHARE "/system/commands/share"
// Where authd keeps the sharing passwords' hashes.
#define SHARING "/system/state/authd/sharing"
#define ALICE 1000
// The folder shared, both ways, and the files the host writes there.
#define PUBLIC "/users/alice/Public"
#define FROM_HOST PUBLIC "/from-host.txt"
#define MADE_BY_HOST PUBLIC "/made-by-host"
#define REFUSED PUBLIC "/refused.txt"
// The host's last file, put when it has checked everything else.
#define DONE PUBLIC "/done.txt"
// How long the host may take, in seconds.
#define HOST_SECONDS 300

// The sharing password alice ends with, which the host signs in with.
#define PASSWORD "through the looking-glass"

// What a program wrote to its standard error.
static char errors[4096];

// Runs `argv` given `input`, as run_program does, with what it writes to
// its standard error in `errors`.
static int run_both(const char *const argv[], const char *input) {
	char path[64];
	snprintf(path, sizeof(path), "/tmp/smbdtest-%d.err", (int)getpid());
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

// Whether something listens on port 445 of the loopback.
static int listening(void) {
	int fd = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
	CHECK(fd >= 0);
	struct sockaddr_in at = {.sin_family = AF_INET, .sin_port = htons(445)};
	at.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
	int connected = connect(fd, (struct sockaddr *)&at, sizeof(at)) == 0;
	close(fd);
	return connected;
}

// Waits up to `seconds` for port 445 to be `open` or not; whether it is.
static int port_becomes(int open, int seconds) {
	long long deadline = check_now_ns() + seconds * 1000000000LL;
	while(listening() != open) {
		if(check_now_ns() > deadline)
			return 0;
		check_sleep_ms(200);
	}
	return 1;
}

// alice's account, and the sharing password root sets her.
static void accounts(void) {
	SAYS(0, "wonderland\n", "Made alice, number 1000, home /users/alice\n", "", USER, "add",
			"alice");
	SAYS(0, "first sharing password\n", "", "The sharing password of alice is set.", SHARE,
			"password", "alice");
	SAYS(1, "x\n", "", "there is no account", SHARE, "password", "nobody-here");
	// Root's, as the built-in accounts', is never set.
	SAYS(1, "x\n", "", "not a person's account", SHARE, "password", "root");
	struct stat st;
	CHECK(stat(SHARING, &st) == 0 && (st.st_mode & 07777) == 0600 && st.st_uid == 0);
	CHECK(!strncmp(contents(SHARING), "alice:", 6));
}

// As alice: she changes her sharing password given her login password,
// and not without it, shares nothing, and reads no sharing password.
static int alice(void) {
	check_start("smbdtest (alice)");
	SAYS(1, "not her password\n" PASSWORD "\n", "", "", SHARE, "password");
	SAYS(0, "wonderland\n" PASSWORD "\n", "", "The sharing password of alice is set.", SHARE,
			"password");
	SAYS(1, "", "", "only root shares folders", SHARE, "add", "Mine", "/users/alice");
	SAYS(1, "", "", "only root turns sharing on", SHARE, "on");
	SAYS(1, "wonderland\nx\n", "", "you change only your own sharing password", SHARE, "password",
			"root");
	CHECK_ERROR(open(SHARING, O_RDONLY | O_CLOEXEC), EACCES);
	SAYS(0, "", "Sharing is off", "", SHARE, "list");
	// Only smbd, as root, signs PCs in, and only root ends the sessions.
	char answer[256];
	read_answer(send_request(REQUEST("share-signin\0" "192.0.2.7\0" "00\0" "00\0" "00\0")), answer,
			sizeof(answer));
	CHECK(strstr(answer, "no only smbd") == answer);
	read_answer(send_request(REQUEST("end-shares\0")), answer, sizeof(answer));
	CHECK(strstr(answer, "no only an administrator") == answer);
	return check_end();
}

// The folder of alice's home that the shares are, and share(1) adding,
// listing and refusing shares.
static void sharing(void) {
	CHECK(mkdir(PUBLIC, 0755) == 0 && chown(PUBLIC, ALICE, ALICE) == 0);
	write_text(PUBLIC "/from-dukovo.txt", "written on Dukovo\n");
	SAYS(0, "", "Shared /users/alice/Public as Public, read-write.", "", SHARE, "add",
			"--comment", "Alice's things", "Public", PUBLIC);
	SAYS(0, "", "Shared /users/alice/Public as Readonly, read-only.", "", SHARE, "add",
			"--read-only", "Readonly", PUBLIC);
	SAYS(1, "", "", "there is a share called public already", SHARE, "add", "public", "/users");
	SAYS(1, "", "", "cannot hold `/`", SHARE, "add", "a/b", PUBLIC);
	SAYS(1, "", "", "IPC$", SHARE, "add", "ipc$", PUBLIC);
	SAYS(1, "", "", "/users/alice/nothing", SHARE, "add", "Nothing", "/users/alice/nothing");
	SAYS(1, "", "", "no share is called Nothing", SHARE, "remove", "Nothing");
	SAYS(1, "", "", "takes a name and a folder", SHARE, "add", "Public");
	SAYS(0, "", "Alice's things", "", SHARE, "list");
	SAYS(0, "", "read-only", "", SHARE);
	SAYS(0, "", "Sharing is off", "", SHARE);
	CHECK(strstr(contents("/system/settings/shares.toml"), "name = \"Readonly\"") != NULL);
}

// `share on` starts smbd, which listens on port 445.
static void on(void) {
	CHECK(!listening());
	SAYS(0, "", "Sharing is on", "", SHARE, "on");
	CHECK(port_becomes(1, 30));
	SAYS(0, "", "Sharing is on", "", SHARE, "list");
	CHECK(strstr(contents("/system/settings/spark/services/smbd.toml"), "\nenabled = true\n") !=
			NULL);
}

// What the host's smbclient did, which it ends by putting done.txt.
static void host(void) {
	printf("smbdtest: ready for the host\n");
	fflush(stdout);
	struct stat st;
	long long deadline = check_now_ns() + HOST_SECONDS * 1000000000LL;
	while(stat(DONE, &st) != 0 && check_now_ns() < deadline)
		check_sleep_ms(500);
	CHECK(stat(DONE, &st) == 0 && st.st_uid == ALICE);
	// A file the host put, and a folder it made, alice's.
	CHECK(!strcmp(contents(FROM_HOST), "written on the host\n"));
	CHECK(stat(FROM_HOST, &st) == 0 && st.st_uid == ALICE && st.st_gid == ALICE);
	CHECK(stat(MADE_BY_HOST, &st) == 0 && S_ISDIR(st.st_mode) && st.st_uid == ALICE);
	// The read-only share wrote nothing.
	CHECK_ERROR(stat(REFUSED, &st), ENOENT);
	// Each sign-in, and the wrong password, as from the share terminal.
	STATUS(0, "/bin/sh", "-c", "last | grep alice | grep -q smb");
	STATUS(0, "/bin/sh", "-c", "lastb | grep alice | grep -q smb");
}

// `share off` stops smbd, which closes the port, and the shares and the
// password are taken away.
static void off(void) {
	SAYS(0, "", "Sharing is off", "", SHARE, "off");
	CHECK(port_becomes(0, 30));
	CHECK(strstr(contents("/system/settings/spark/services/smbd.toml"), "\nenabled = false\n") !=
			NULL);
	SAYS(0, "", "Stopped sharing Readonly", "", SHARE, "remove", "readonly");
	SAYS(0, "", "Stopped sharing Public", "", SHARE, "remove", "Public");
	SAYS(0, "", "No folder is shared", "", SHARE, "list");
	SAYS(0, "", "", "The sharing password of alice is cleared.", SHARE, "password", "--clear",
			"alice");
	CHECK(strstr(contents(SHARING), "alice:") == NULL);
}

int main(int argc, char **argv) {
	if(argc == 2 && !strcmp(argv[1], "alice"))
		return alice();
	check_start("smbdtest");
	authd_answers();
	RUN(accounts);
	// She notes the commands she runs, for the `commands` test, too.
	int notes = open(COMMANDS_RUN, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0644);
	CHECK(notes >= 0 && fchmod(notes, 0666) == 0);
	close(notes);
	char *const alice_args[] = {"smbdtest", "alice", NULL};
	pid_t pid = start_as(PROGRAM, ALICE, ALICE, NULL, 0, alice_args);
	CHECK_CHILD(pid);
	RUN(sharing);
	RUN(on);
	RUN(host);
	RUN(off);
	return check_end();
}
