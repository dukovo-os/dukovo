// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Linux's seccomp (ADR 0623), as a Linux program uses it: what each
// action a filter returns does to the call, the SIGSYS record of a TRAP
// and the frame its handler gets, the clamp of a large errno, the
// arguments a filter reads, a chain of filters where the strictest value
// wins, threads given one chain together (TSYNC) and the thread number or
// ESRCH when one of them cannot take it, strict mode's four calls and the
// SIGKILL for any other, a call that linuxd answers refused as the
// kernel's own are, a filter kept by fork, by new threads and by execve
// (the program runs itself with --inherited), the EACCES of a filter
// without no_new_privs, the EINVAL of each program shape Linux refuses,
// and the ENOMEM past 32,768 instructions along a chain.
//
// It is a Linux program, never one of Dukovo's: the linux-programs boot
// test builds it with the build machine's C library and with musl and
// runs both on Dukovo. It passes on a Linux machine too, which is how its
// expectations are known to be Linux's. Where Dukovo answers otherwise by
// design (a filtered program may not start a Dukovo one; no listener for
// USER_NOTIF is made), the program checks Dukovo's answer when `uname -v`
// says Dukovo and Linux's elsewhere.
//
// /proc/self/status names the seccomp mode, the filters and no_new_privs.
// Dukovo's Linux view has /proc, from linuxd; a Linux program run by its
// name has none, so there the checks of the status lines are left out, and
// the boot test runs `seccomp --status` through `linux run` to make them.
//
// Every filter is installed in a child of its own, as nothing takes one
// away.

#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <pthread.h>
#include <stdint.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/ucontext.h>
#include <sys/utsname.h>

#include "check.h"

// struct sock_filter and struct sock_fprog (linux/filter.h), here so that
// the program builds with musl, whose headers do not carry Linux's.
struct insn {
	uint16_t code;
	uint8_t jt;
	uint8_t jf;
	uint32_t k;
};
struct fprog {
	unsigned short len;
	const struct insn *filter;
};

// The instructions the filters here are made of (linux/bpf_common.h).
#define LD_W_ABS 0x20 // BPF_LD | BPF_W | BPF_ABS
#define LD_H_ABS 0x28 // BPF_LD | BPF_H | BPF_ABS
#define LD_B_ABS 0x30 // BPF_LD | BPF_B | BPF_ABS
#define LD_W_IND 0x40 // BPF_LD | BPF_W | BPF_IND
#define LD_W_MEM 0x60 // BPF_LD | BPF_W | BPF_MEM
#define LD_W_LEN 0x80 // BPF_LD | BPF_W | BPF_LEN
#define LDX_MSH 0xb1 // BPF_LDX | BPF_B | BPF_MSH
#define ST 0x02 // BPF_ST
#define ALU_OR_K 0x44 // BPF_ALU | BPF_OR | BPF_K
#define ALU_DIV_K 0x34 // BPF_ALU | BPF_DIV | BPF_K
#define ALU_MOD_K 0x94 // BPF_ALU | BPF_MOD | BPF_K
#define JA 0x05 // BPF_JMP | BPF_JA
#define JEQ_K 0x15 // BPF_JMP | BPF_JEQ | BPF_K
#define RET_K 0x06 // BPF_RET | BPF_K
#define RET_A 0x16 // BPF_RET | BPF_A

#define STMT(c, value) {(uint16_t)(c), 0, 0, (uint32_t)(value)}
#define JUMP(c, value, t, f) {(uint16_t)(c), (t), (f), (uint32_t)(value)}
#define COUNT(array) (sizeof(array) / sizeof((array)[0]))

// Where struct seccomp_data keeps each field.
#define DATA_NR 0
#define DATA_ARCH 4
#define DATA_IP 8
#define DATA_ARG_LOW(n) (16 + 8 * (n))
#define DATA_ARG_HIGH(n) (20 + 8 * (n))
#define DATA_SIZE 64

// linux/seccomp.h and linux/audit.h.
#define AUDIT_ARCH_X86_64 0xc000003eU
#define RET_KILL_PROCESS 0x80000000U
#define RET_KILL_THREAD 0x00000000U
#define RET_TRAP 0x00030000U
#define RET_ERRNO 0x00050000U
#define RET_USER_NOTIF 0x7fc00000U
#define RET_TRACE 0x7ff00000U
#define RET_LOG 0x7ffc0000U
#define RET_ALLOW 0x7fff0000U
#define SET_MODE_STRICT 0
#define SET_MODE_FILTER 1
#define GET_ACTION_AVAIL 2
#define GET_NOTIF_SIZES 3
#define FLAG_TSYNC 1U
#define FLAG_LOG 2U
#define FLAG_SPEC_ALLOW 4U
#define FLAG_NEW_LISTENER 8U
#define FLAG_TSYNC_ESRCH 16U
#define MODE_STRICT 1
#define MODE_FILTER 2
#define CODE_SYS_SECCOMP 1

#ifndef SYS_seccomp
#define SYS_seccomp 317
#endif

// The longest program Linux takes, and the most instructions along a
// chain, counting four more for each filter before the newest.
#define MAX_INSTRUCTIONS 4096
#define MAX_INSTRUCTIONS_PER_PATH 32768

// Whether the program runs on Dukovo.
static int on_dukovo;

// The program's own path, which `inherited` runs.
static const char *self;

// Whether the process has /proc/self/status: always on Linux, and on
// Dukovo in the Linux view.
static int has_proc;

static long seccomp_call(unsigned operation, unsigned flags, const void *args) {
	return syscall(SYS_seccomp, operation, flags, args);
}

// Installs the `len` instructions of `insns` with `flags`; returns what
// seccomp did.
static long install(const struct insn *insns, size_t len, unsigned flags) {
	struct fprog prog = {(unsigned short)len, insns};
	return seccomp_call(SET_MODE_FILTER, flags, &prog);
}

// Installs a filter that gives call `nr` the value `value` and allows
// every other call. A call made as another arch than x86-64 ends the
// process, as every real filter's first lines say.
static long refuse(long nr, uint32_t value, unsigned flags) {
	const struct insn insns[] = {
			STMT(LD_W_ABS, DATA_ARCH),
			JUMP(JEQ_K, AUDIT_ARCH_X86_64, 1, 0),
			STMT(RET_K, RET_KILL_PROCESS),
			STMT(LD_W_ABS, DATA_NR),
			JUMP(JEQ_K, nr, 0, 1),
			STMT(RET_K, value),
			STMT(RET_K, RET_ALLOW),
	};
	return install(insns, COUNT(insns), flags);
}

static int set_no_new_privs(void) {
	return prctl(PR_SET_NO_NEW_PRIVS, 1UL, 0UL, 0UL, 0UL);
}

static int get_no_new_privs(void) {
	return prctl(PR_GET_NO_NEW_PRIVS, 0UL, 0UL, 0UL, 0UL);
}

static int mode(void) {
	return prctl(PR_GET_SECCOMP, 0UL, 0UL, 0UL, 0UL);
}

static long parent_of_me(void) {
	return syscall(SYS_getppid);
}

// Runs `part` in a child, which exits with how many of its checks failed,
// and returns the child's wait status.
static int in_child(void (*part)(void)) {
	pid_t pid = check_fork();
	if(pid == 0) {
		part();
		check_exit_child();
	}
	return check_wait(pid);
}

// Runs `part` in a child, and checks that all of its checks passed.
#define CHILD(part) \
	do { \
		int child_status_ = in_child(part); \
		if(!WIFEXITED(child_status_) || WEXITSTATUS(child_status_) != 0) { \
			printf("%s: %s ended with status %#x\n", check_name, #part, child_status_); \
			check_failed(__FILE__, __LINE__, #part " passed", 0); \
		} \
	} while(0)

// Runs `part` in a child, and checks that a signal ended it.
#define CHILD_KILLED(part, signal) \
	do { \
		int child_status_ = in_child(part); \
		if(!WIFSIGNALED(child_status_) || WTERMSIG(child_status_) != (signal)) { \
			printf("%s: %s ended with status %#x\n", check_name, #part, child_status_); \
			check_failed(__FILE__, __LINE__, #part " was ended by " #signal, 0); \
		} \
	} while(0)

// Whether /proc/self/status has the line `line`.
static int status_says(const char *line) {
	FILE *status = fopen("/proc/self/status", "r");
	if(!status)
		return 0;
	char text[256];
	int found = 0;
	size_t want = strlen(line);
	while(!found && fgets(text, sizeof(text), status))
		found = strncmp(text, line, want) == 0 && text[want] == '\n';
	fclose(status);
	return found;
}

// Checks that /proc/self/status has the line `line`, where there is one.
#define CHECK_STATUS(line) \
	do { \
		if(has_proc) \
			CHECK(status_says(line)); \
	} while(0)

// --- What a process with no filter sees ---

// The process has no filter, and the kernel names the eight actions it
// knows and no other value.
static void before(void) {
	CHECK(mode() == 0);
	const uint32_t known[] = {RET_KILL_PROCESS, RET_KILL_THREAD, RET_TRAP, RET_ERRNO,
			RET_USER_NOTIF, RET_TRACE, RET_LOG, RET_ALLOW};
	for(size_t i = 0; i < COUNT(known); i++)
		CHECK(seccomp_call(GET_ACTION_AVAIL, 0, &known[i]) == 0);
	// A value with data is not an action, nor is one Linux never made.
	uint32_t with_data = RET_ERRNO | 5, unknown = 0x00010000U;
	CHECK_ERROR(seccomp_call(GET_ACTION_AVAIL, 0, &with_data), EOPNOTSUPP);
	CHECK_ERROR(seccomp_call(GET_ACTION_AVAIL, 0, &unknown), EOPNOTSUPP);
	CHECK_ERROR(seccomp_call(GET_ACTION_AVAIL, 1, &known[0]), EINVAL);
	CHECK_ERROR(seccomp_call(GET_ACTION_AVAIL, 0, (void *)8), EFAULT);
	CHECK_ERROR(seccomp_call(99, 0, NULL), EINVAL);
	CHECK_ERROR(seccomp_call(SET_MODE_STRICT, 1, NULL), EINVAL);
	CHECK_ERROR(seccomp_call(SET_MODE_STRICT, 0, &known[0]), EINVAL);
	uint16_t sizes[3];
	if(on_dukovo)
		CHECK_ERROR(seccomp_call(GET_NOTIF_SIZES, 0, sizes), EINVAL);
	else
		CHECK(seccomp_call(GET_NOTIF_SIZES, 0, sizes) == 0);
	CHECK_STATUS("Seccomp:\t0");
}

// --- The status lines ---

// What /proc/self/status says of a process with neither no_new_privs nor
// a filter, then with each, and of its child, which keeps both.
static void status_lines(void) {
	CHECK(get_no_new_privs() == 0 && mode() == 0);
	CHECK(status_says("NoNewPrivs:\t0"));
	CHECK(status_says("Seccomp:\t0"));
	CHECK(status_says("Seccomp_filters:\t0"));
	CHECK(set_no_new_privs() == 0);
	CHECK(status_says("NoNewPrivs:\t1"));
	CHECK(refuse(SYS_getppid, RET_ERRNO | EXDEV, 0) == 0);
	CHECK(status_says("Seccomp:\t2"));
	CHECK(status_says("Seccomp_filters:\t1"));
	pid_t pid = check_fork();
	if(pid == 0) {
		CHECK(status_says("NoNewPrivs:\t1"));
		CHECK(status_says("Seccomp:\t2"));
		CHECK(status_says("Seccomp_filters:\t1"));
		check_exit_child();
	}
	CHECK_CHILD(pid);
}

// --- no_new_privs ---

static void privileges(void) {
	// A Linux machine's root may install a filter without no_new_privs:
	// one that is not may not, as no Linux program on Dukovo may.
	if(!on_dukovo && geteuid() == 0)
		CHECK(setresuid(65534, 65534, 65534) == 0);
	CHECK(get_no_new_privs() == 0);
	CHECK_ERROR(refuse(SYS_getppid, RET_ERRNO | EXDEV, 0), EACCES);
	CHECK(mode() == 0);
	CHECK_ERROR(prctl(PR_SET_NO_NEW_PRIVS, 2UL, 0UL, 0UL, 0UL), EINVAL);
	CHECK_ERROR(prctl(PR_SET_NO_NEW_PRIVS, 1UL, 1UL, 0UL, 0UL), EINVAL);
	CHECK_ERROR(prctl(PR_GET_NO_NEW_PRIVS, 1UL, 0UL, 0UL, 0UL), EINVAL);
	CHECK(get_no_new_privs() == 0);
	CHECK(set_no_new_privs() == 0);
	CHECK(get_no_new_privs() == 1);
	CHECK_STATUS("NoNewPrivs:\t1");
	// Set again, it stays.
	CHECK(set_no_new_privs() == 0);
	pid_t pid = check_fork();
	if(pid == 0) {
		CHECK(get_no_new_privs() == 1);
		check_exit_child();
	}
	CHECK_CHILD(pid);
	CHECK(refuse(SYS_getppid, RET_ERRNO | EXDEV, 0) == 0);
	CHECK(mode() == MODE_FILTER);
}

// --- Actions ---

static void errno_action(void) {
	long parent = parent_of_me();
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_ERRNO | EXDEV, 0) == 0);
	CHECK(mode() == MODE_FILTER);
	CHECK_STATUS("Seccomp:\t2");
	CHECK_STATUS("Seccomp_filters:\t1");
	CHECK_ERROR(parent_of_me(), EXDEV);
	// The other calls are made.
	CHECK(getpid() > 0 && getpid() != parent);
}

static void errno_zero(void) {
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_ERRNO | 0, 0) == 0);
	errno = 0;
	CHECK(parent_of_me() == 0 && errno == 0);
}

// An errno past Linux's largest is that largest, 4095.
static void errno_clamped(void) {
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_ERRNO | 5000, 0) == 0);
	CHECK_ERROR(parent_of_me(), 4095);
}

static void allow_and_log(void) {
	long parent = parent_of_me();
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_ALLOW, 0) == 0);
	CHECK(refuse(SYS_getppid, RET_LOG, FLAG_LOG | FLAG_SPEC_ALLOW) == 0);
	CHECK(parent_of_me() == parent);
}

// TRACE with no tracer, and USER_NOTIF with no listener, fail the call
// with ENOSYS.
static void trace_and_notify(void) {
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_TRACE | 7, 0) == 0);
	CHECK_ERROR(parent_of_me(), ENOSYS);
	CHECK(refuse(SYS_getpgid, RET_USER_NOTIF, 0) == 0);
	CHECK_ERROR(syscall(SYS_getpgid, 0), ENOSYS);
}

// What the SIGSYS handler saw.
static volatile int trap_signal, trap_code, trap_errno, trap_syscall;
static volatile unsigned trap_arch;
static void *volatile trap_address;
static volatile long trap_rax, trap_rdi;
static volatile unsigned long trap_rip;

// Notes what the SIGSYS says, and makes the call that was not made return
// 1234, as a program that emulates the call does.
static void on_sigsys(int signal, siginfo_t *info, void *context) {
	ucontext_t *uc = context;
	trap_signal = signal;
	trap_code = info->si_code;
	trap_errno = info->si_errno;
	trap_syscall = info->si_syscall;
	trap_arch = info->si_arch;
	trap_address = info->si_call_addr;
	trap_rax = uc->uc_mcontext.gregs[REG_RAX];
	trap_rdi = uc->uc_mcontext.gregs[REG_RDI];
	trap_rip = (unsigned long)uc->uc_mcontext.gregs[REG_RIP];
	uc->uc_mcontext.gregs[REG_RAX] = 1234;
}

static void trap(void) {
	struct sigaction action = {0};
	action.sa_sigaction = on_sigsys;
	action.sa_flags = SA_SIGINFO;
	CHECK(sigaction(SIGSYS, &action, NULL) == 0);
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_TRAP | 0x42, 0) == 0);
	CHECK(syscall(SYS_getppid, 77L) == 1234);
	CHECK(trap_signal == SIGSYS);
	CHECK(trap_code == CODE_SYS_SECCOMP);
	CHECK(trap_errno == 0x42);
	CHECK(trap_syscall == SYS_getppid);
	CHECK(trap_arch == AUDIT_ARCH_X86_64);
	// The call is rolled back: its number in rax, its arguments where they
	// were, and the address after its syscall instruction both in the
	// record and in the frame.
	CHECK(trap_rax == SYS_getppid);
	CHECK(trap_rdi == 77);
	CHECK(trap_address != NULL && (unsigned long)trap_address == trap_rip);
	// The handler ran with SIGSYS blocked, and returned: it is unblocked
	// again.
	sigset_t now;
	CHECK(sigprocmask(SIG_BLOCK, NULL, &now) == 0);
	CHECK(!sigismember(&now, SIGSYS));
}

// A TRAP whose SIGSYS is blocked or ignored, or has no handler, ends the
// process with SIGSYS.
static void trap_blocked(void) {
	struct sigaction action = {0};
	action.sa_sigaction = on_sigsys;
	action.sa_flags = SA_SIGINFO;
	CHECK(sigaction(SIGSYS, &action, NULL) == 0);
	sigset_t sys;
	sigemptyset(&sys);
	sigaddset(&sys, SIGSYS);
	CHECK(sigprocmask(SIG_BLOCK, &sys, NULL) == 0);
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_TRAP, 0) == 0);
	parent_of_me();
	printf("%s: trap_blocked: the call returned\n", check_name);
}

static void trap_ignored(void) {
	signal(SIGSYS, SIG_IGN);
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_TRAP, 0) == 0);
	parent_of_me();
	printf("%s: trap_ignored: the call returned\n", check_name);
}

static void trap_default(void) {
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_TRAP, 0) == 0);
	parent_of_me();
	printf("%s: trap_default: the call returned\n", check_name);
}

static void kill_process(void) {
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_KILL_PROCESS, 0) == 0);
	parent_of_me();
	printf("%s: kill_process: the call returned\n", check_name);
}

// A value whose action Linux does not know ends the process too.
static void unknown_action(void) {
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, 0x00010000U, 0) == 0);
	parent_of_me();
	printf("%s: unknown_action: the call returned\n", check_name);
}

// KILL_THREAD in the only thread ends the process.
static void kill_only_thread(void) {
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_KILL_THREAD, 0) == 0);
	parent_of_me();
	printf("%s: kill_only_thread: the call returned\n", check_name);
}

// The number of the thread a test made.
static volatile long thread_id;

// Waits, for at most ten seconds, until the thread `thread_id` names has
// ended, which a thread a filter or strict mode ended has not told its C
// library: musl's pthread_join would wait for ever.
static void wait_ended(void) {
	for(int tries = 0; tries < 1000; tries++) {
		if(syscall(SYS_tgkill, (long)getpid(), thread_id, 0L) < 0 && errno == ESRCH)
			return;
		check_sleep_ms(10);
	}
	check_failed(__FILE__, __LINE__, "the thread ended", 0);
}

static void *call_getppid(void *unused) {
	(void)unused;
	thread_id = syscall(SYS_gettid);
	parent_of_me();
	return (void *)1;
}

// KILL_THREAD in one of two threads ends that thread, and the other goes
// on.
static void kill_one_thread(void) {
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_KILL_THREAD, 0) == 0);
	thread_id = 0;
	pthread_t thread;
	CHECK(pthread_create(&thread, NULL, call_getppid, NULL) == 0);
	for(int tries = 0; tries < 1000 && thread_id == 0; tries++)
		check_sleep_ms(10);
	CHECK(thread_id != 0);
	wait_ended();
	CHECK(getpid() > 0);
}

// KILL_PROCESS in one of two threads ends both.
static void kill_from_thread(void) {
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_KILL_PROCESS, 0) == 0);
	pthread_t thread;
	CHECK(pthread_create(&thread, NULL, call_getppid, NULL) == 0);
	pthread_join(thread, NULL);
	printf("%s: kill_from_thread: the thread was joined\n", check_name);
}

// --- What filters read ---

// A filter reads both halves of an argument, the length of the record,
// and its scratch memory, and returns a value it computed.
static void arguments(void) {
	long parent = parent_of_me();
	const struct insn insns[] = {
			STMT(LD_W_ABS, DATA_ARCH),
			JUMP(JEQ_K, AUDIT_ARCH_X86_64, 1, 0),
			STMT(RET_K, RET_KILL_PROCESS),
			STMT(LD_W_ABS, DATA_NR),
			JUMP(JEQ_K, SYS_getppid, 1, 0),
			STMT(RET_K, RET_ALLOW),
			// The record is 64 bytes.
			STMT(LD_W_LEN, 0),
			JUMP(JEQ_K, DATA_SIZE, 1, 0),
			STMT(RET_K, RET_ERRNO | EPERM),
			// The call's address is not 0.
			STMT(LD_W_ABS, DATA_IP),
			JUMP(JEQ_K, 0, 0, 1),
			STMT(RET_K, RET_ERRNO | EPERM),
			// The high half of the first argument chooses.
			STMT(LD_W_ABS, DATA_ARG_HIGH(0)),
			JUMP(JEQ_K, 0x1234, 1, 0),
			STMT(RET_K, RET_ALLOW),
			// Its low half, through memory, is the errno.
			STMT(LD_W_ABS, DATA_ARG_LOW(0)),
			STMT(ST, 3),
			STMT(LD_W_MEM, 3),
			STMT(ALU_OR_K, RET_ERRNO),
			STMT(RET_A, 0),
	};
	CHECK(set_no_new_privs() == 0);
	CHECK(install(insns, COUNT(insns), 0) == 0);
	CHECK(syscall(SYS_getppid, 7L) == parent);
	CHECK(syscall(SYS_getppid, 0x0000123500000007L) == parent);
	CHECK_ERROR(syscall(SYS_getppid, 0x0000123400000007L), E2BIG);
	CHECK_ERROR(syscall(SYS_getppid, 0x0000123400000000L | EXDEV), EXDEV);
}

// Of a chain's values, the strictest wins, and of two with the same
// action the newest filter's.
static void chain(void) {
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_ERRNO | EPERM, 0) == 0);
	CHECK(refuse(SYS_getppid, RET_ALLOW, 0) == 0);
	CHECK_ERROR(parent_of_me(), EPERM);
	CHECK(refuse(SYS_getppid, RET_TRACE, 0) == 0);
	CHECK_ERROR(parent_of_me(), EPERM);
	CHECK(refuse(SYS_getppid, RET_ERRNO | EXDEV, 0) == 0);
	CHECK_ERROR(parent_of_me(), EXDEV);
	CHECK(refuse(SYS_getppid, RET_ERRNO | E2BIG, 0) == 0);
	CHECK_ERROR(parent_of_me(), E2BIG);
	CHECK_STATUS("Seccomp_filters:\t5");
	// prctl adds one the same way.
	const struct insn insns[] = {
			STMT(LD_W_ABS, DATA_NR),
			JUMP(JEQ_K, SYS_getppid, 0, 1),
			STMT(RET_K, RET_ERRNO | ENOTTY),
			STMT(RET_K, RET_ALLOW),
	};
	struct fprog prog = {COUNT(insns), insns};
	CHECK(prctl(PR_SET_SECCOMP, (unsigned long)MODE_FILTER, &prog, 0UL, 0UL) == 0);
	CHECK_ERROR(parent_of_me(), ENOTTY);
	CHECK_ERROR(prctl(PR_SET_SECCOMP, 3UL, &prog, 0UL, 0UL), EINVAL);
	// With a filter, strict mode is refused.
	CHECK_ERROR(seccomp_call(SET_MODE_STRICT, 0, NULL), EINVAL);
	CHECK_ERROR(prctl(PR_SET_SECCOMP, (unsigned long)MODE_STRICT, 0UL, 0UL, 0UL), EINVAL);
	CHECK(mode() == MODE_FILTER);
}

// A call linuxd answers for the kernel is judged before it goes there.
static void forwarded(void) {
	CHECK(set_no_new_privs() == 0);
	const struct insn insns[] = {
			STMT(LD_W_ABS, DATA_NR),
			JUMP(JEQ_K, SYS_socket, 0, 3),
			STMT(LD_W_ABS, DATA_ARG_LOW(0)),
			JUMP(JEQ_K, AF_NETLINK, 0, 1),
			STMT(RET_K, RET_ERRNO | EXDEV),
			STMT(RET_K, RET_ALLOW),
	};
	CHECK(install(insns, COUNT(insns), 0) == 0);
	CHECK_ERROR(socket(AF_NETLINK, SOCK_RAW, 0), EXDEV);
	int unix_socket = socket(AF_UNIX, SOCK_STREAM, 0);
	CHECK(unix_socket >= 0);
	close(unix_socket);
}

// --- What a filter stays with ---

// A child, a new thread and a program started with execve keep the
// filters.
static void *thread_inherits(void *unused) {
	(void)unused;
	CHECK(mode() == MODE_FILTER);
	CHECK(get_no_new_privs() == 1);
	CHECK_ERROR(parent_of_me(), EXDEV);
	return NULL;
}

static void inheritance(void) {
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_ERRNO | EXDEV, 0) == 0);
	pthread_t thread;
	CHECK(pthread_create(&thread, NULL, thread_inherits, NULL) == 0);
	CHECK(pthread_join(thread, NULL) == 0);
	pid_t pid = check_fork();
	if(pid == 0) {
		CHECK(mode() == MODE_FILTER);
		CHECK(get_no_new_privs() == 1);
		CHECK_ERROR(parent_of_me(), EXDEV);
		check_exit_child();
	}
	CHECK_CHILD(pid);
	pid = check_fork();
	if(pid == 0) {
		execl(self, self, "--inherited", (char *)NULL);
		check_failed(__FILE__, __LINE__, "execl(self, --inherited)", errno);
		check_exit_child();
	}
	CHECK_CHILD(pid);
}

// What the program checks when it runs itself with --inherited.
static int inherited(void) {
	check_start("seccomp --inherited");
	CHECK(mode() == MODE_FILTER);
	CHECK(get_no_new_privs() == 1);
	CHECK_ERROR(parent_of_me(), EXDEV);
	CHECK_STATUS("Seccomp_filters:\t1");
	return check_end();
}

// A filtered program may not start a Dukovo one, which would leave the
// filter behind; on Linux, every program is one the filter judges.
static void native_program(void) {
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_ERRNO | EXDEV, 0) == 0);
	if(on_dukovo) {
		char *argv[] = {"/system/commands/true", NULL};
		CHECK_ERROR(execv(argv[0], argv), EACCES);
	}
}

// --- Threads synchronized ---

// The two ends of a pipe that tells a thread to go on, and one the thread
// says it is ready on.
static int go[2], ready[2];
static volatile int seen_privs_before, seen_mode, seen_privs, seen_errno;

static void wait_for_go(void) {
	char byte;
	while(syscall(SYS_read, go[0], &byte, 1) < 0 && errno == EINTR) {
	}
}

static void say_ready(void) {
	char byte = 'r';
	syscall(SYS_write, ready[1], &byte, 1);
}

static void wait_ready(void) {
	char byte;
	while(read(ready[0], &byte, 1) < 0 && errno == EINTR) {
	}
}

static void let_go(void) {
	char byte = 'g';
	CHECK(write(go[1], &byte, 1) == 1);
}

static void *synced(void *unused) {
	(void)unused;
	seen_privs_before = get_no_new_privs();
	say_ready();
	wait_for_go();
	seen_mode = mode();
	seen_privs = get_no_new_privs();
	errno = 0;
	parent_of_me();
	seen_errno = errno;
	return NULL;
}

// TSYNC gives every thread the caller's new chain, and its
// no_new_privs.
static void tsync(void) {
	CHECK(pipe(go) == 0 && pipe(ready) == 0);
	pthread_t thread;
	CHECK(pthread_create(&thread, NULL, synced, NULL) == 0);
	wait_ready();
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_ERRNO | EXDEV, FLAG_TSYNC) == 0);
	let_go();
	CHECK(pthread_join(thread, NULL) == 0);
	CHECK(seen_privs_before == 0);
	CHECK(seen_mode == MODE_FILTER);
	CHECK(seen_privs == 1);
	CHECK(seen_errno == EXDEV);
}

static void *filtered_alone(void *unused) {
	(void)unused;
	thread_id = syscall(SYS_gettid);
	set_no_new_privs();
	refuse(SYS_getpgid, RET_ERRNO | EPERM, 0);
	say_ready();
	wait_for_go();
	return NULL;
}

// A thread whose chain is not one the caller's grew from stops a TSYNC:
// nothing changes, and the answer is its number, or ESRCH.
static void tsync_refused(void) {
	long parent = parent_of_me();
	CHECK(pipe(go) == 0 && pipe(ready) == 0);
	pthread_t thread;
	CHECK(pthread_create(&thread, NULL, filtered_alone, NULL) == 0);
	wait_ready();
	CHECK(set_no_new_privs() == 0);
	long got = refuse(SYS_getppid, RET_ERRNO | EXDEV, FLAG_TSYNC);
	CHECK(got == thread_id && got > 0);
	CHECK(mode() == 0);
	CHECK(parent_of_me() == parent);
	CHECK_ERROR(refuse(SYS_getppid, RET_ERRNO | EXDEV, FLAG_TSYNC | FLAG_TSYNC_ESRCH), ESRCH);
	CHECK(mode() == 0);
	// Without TSYNC, the caller takes it alone.
	CHECK(refuse(SYS_getppid, RET_ERRNO | EXDEV, 0) == 0);
	CHECK_ERROR(parent_of_me(), EXDEV);
	let_go();
	CHECK(pthread_join(thread, NULL) == 0);
}

static void *strict_alone(void *unused) {
	(void)unused;
	thread_id = syscall(SYS_gettid);
	syscall(SYS_seccomp, SET_MODE_STRICT, 0, NULL);
	// Only read, write and exit from here.
	say_ready();
	wait_for_go();
	syscall(SYS_exit, 0);
	return NULL;
}

// A thread in strict mode stops a TSYNC too.
static void tsync_strict(void) {
	CHECK(pipe(go) == 0 && pipe(ready) == 0);
	pthread_t thread;
	CHECK(pthread_create(&thread, NULL, strict_alone, NULL) == 0);
	wait_ready();
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_ERRNO | EXDEV, FLAG_TSYNC) == thread_id);
	CHECK(mode() == 0);
	let_go();
	wait_ended();
}

// A thread filtered by the same chain as the caller's takes the new
// one.
static void *filtered_too(void *unused) {
	(void)unused;
	say_ready();
	wait_for_go();
	errno = 0;
	syscall(SYS_getpgid, 0);
	seen_errno = errno;
	return NULL;
}

static void tsync_shared(void) {
	CHECK(pipe(go) == 0 && pipe(ready) == 0);
	CHECK(set_no_new_privs() == 0);
	CHECK(refuse(SYS_getppid, RET_ERRNO | EXDEV, 0) == 0);
	pthread_t thread;
	CHECK(pthread_create(&thread, NULL, filtered_too, NULL) == 0);
	wait_ready();
	CHECK(refuse(SYS_getpgid, RET_ERRNO | E2BIG, FLAG_TSYNC) == 0);
	let_go();
	CHECK(pthread_join(thread, NULL) == 0);
	CHECK(seen_errno == E2BIG);
}

// --- Strict mode ---

// What a child in strict mode did, written to its pipe in one write.
struct strict_report {
	long entered;
	long wrote;
};

static int report[2];

// Enters strict mode with seccomp or prctl, writes a report with the
// write call, then ends the way `ending` says.
enum ending { END_WITH_EXIT, END_WITH_EXIT_GROUP, END_WITH_GETPPID, END_WITH_PRCTL };
static enum ending ending;
static int strict_with_prctl;

static void strict(void) {
	struct strict_report done;
	if(strict_with_prctl)
		done.entered = syscall(SYS_prctl, PR_SET_SECCOMP, (long)MODE_STRICT, 0L, 0L, 0L);
	else
		done.entered = syscall(SYS_seccomp, SET_MODE_STRICT, 0, NULL);
	done.wrote = syscall(SYS_write, report[1], "w", 1);
	syscall(SYS_write, report[1], &done, sizeof(done));
	switch(ending) {
	case END_WITH_EXIT:
		syscall(SYS_exit, 0);
		break;
	case END_WITH_EXIT_GROUP:
		syscall(SYS_exit_group, 0);
		break;
	case END_WITH_GETPPID:
		syscall(SYS_getppid);
		break;
	case END_WITH_PRCTL:
		syscall(SYS_prctl, PR_GET_SECCOMP, 0L, 0L, 0L, 0L);
		break;
	}
	syscall(SYS_exit, 3);
}

// Runs `strict` in a child that ends the way `how` says, and returns its
// wait status, having checked its report.
static int strict_child(enum ending how, int with_prctl) {
	ending = how;
	strict_with_prctl = with_prctl;
	CHECK(pipe(report) == 0);
	int status = in_child(strict);
	close(report[1]);
	char byte = 0;
	struct strict_report done = {-1, -1};
	CHECK(read(report[0], &byte, 1) == 1 && byte == 'w');
	CHECK(read(report[0], &done, sizeof(done)) == (ssize_t)sizeof(done));
	CHECK(done.entered == 0);
	CHECK(done.wrote == 1);
	close(report[0]);
	return status;
}

// Strict mode needs no no_new_privs, allows read, write, exit and
// rt_sigreturn, and ends the thread with SIGKILL for any other call, even
// exit_group and prctl.
static void strict_mode(void) {
	for(int with_prctl = 0; with_prctl < 2; with_prctl++) {
		int status = strict_child(END_WITH_EXIT, with_prctl);
		CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
		status = strict_child(END_WITH_GETPPID, with_prctl);
		CHECK(WIFSIGNALED(status) && WTERMSIG(status) == SIGKILL);
	}
	int status = strict_child(END_WITH_EXIT_GROUP, 0);
	CHECK(WIFSIGNALED(status) && WTERMSIG(status) == SIGKILL);
	status = strict_child(END_WITH_PRCTL, 0);
	CHECK(WIFSIGNALED(status) && WTERMSIG(status) == SIGKILL);
}

// --- Programs refused ---

// Each shape of program Linux refuses gets EINVAL, a program it cannot
// read EFAULT, and no filter is installed.
static void refused(void) {
	CHECK(set_no_new_privs() == 0);
	struct fprog none = {0, NULL};
	CHECK_ERROR(seccomp_call(SET_MODE_FILTER, 0, &none), EINVAL);
	CHECK_ERROR(seccomp_call(SET_MODE_FILTER, 0, (void *)8), EFAULT);
	struct fprog unreadable = {1, (const struct insn *)8};
	CHECK_ERROR(seccomp_call(SET_MODE_FILTER, 0, &unreadable), EFAULT);
	static struct insn longest[MAX_INSTRUCTIONS + 1];
	for(size_t i = 0; i < COUNT(longest); i++)
		longest[i] = (struct insn)STMT(RET_K, RET_ALLOW);
	CHECK_ERROR(install(longest, COUNT(longest), 0), EINVAL);
	const struct insn allow = STMT(RET_K, RET_ALLOW);
	CHECK_ERROR(install(&allow, 1, 1U << 10), EINVAL);
	if(on_dukovo) {
		CHECK_ERROR(install(&allow, 1, FLAG_NEW_LISTENER), EINVAL);
		CHECK_ERROR(install(&allow, 1, FLAG_TSYNC | FLAG_NEW_LISTENER), EINVAL);
	}

	// Each refused program: what it does wrong, then its instructions.
	const struct insn no_return[] = {STMT(LD_W_ABS, DATA_NR)};
	const struct insn jumps_out[] = {JUMP(JEQ_K, 0, 5, 0), STMT(RET_K, RET_ALLOW)};
	const struct insn past_the_record[] = {STMT(LD_W_ABS, DATA_SIZE), STMT(RET_K, RET_ALLOW)};
	const struct insn unaligned[] = {STMT(LD_W_ABS, 2), STMT(RET_K, RET_ALLOW)};
	const struct insn half_word[] = {STMT(LD_H_ABS, 0), STMT(RET_K, RET_ALLOW)};
	const struct insn byte[] = {STMT(LD_B_ABS, 0), STMT(RET_K, RET_ALLOW)};
	const struct insn indirect[] = {STMT(LD_W_IND, 0), STMT(RET_K, RET_ALLOW)};
	const struct insn header_length[] = {STMT(LDX_MSH, 0), STMT(RET_K, RET_ALLOW)};
	const struct insn modulo[] = {STMT(ALU_MOD_K, 3), STMT(RET_K, RET_ALLOW)};
	const struct insn by_zero[] = {STMT(ALU_DIV_K, 0), STMT(RET_K, RET_ALLOW)};
	const struct insn past_memory[] = {STMT(ST, 16), STMT(RET_K, RET_ALLOW)};
	const struct insn unwritten[] = {STMT(LD_W_MEM, 0), STMT(RET_K, RET_ALLOW)};
	const struct insn unknown[] = {STMT(0xffff, 0), STMT(RET_K, RET_ALLOW)};
	const struct {
		const char *what;
		const struct insn *insns;
		size_t len;
	} programs[] = {
			{"no return", no_return, COUNT(no_return)},
			{"a jump out", jumps_out, COUNT(jumps_out)},
			{"a load past the record", past_the_record, COUNT(past_the_record)},
			{"an unaligned load", unaligned, COUNT(unaligned)},
			{"a load of a half word", half_word, COUNT(half_word)},
			{"a load of a byte", byte, COUNT(byte)},
			{"an indirect load", indirect, COUNT(indirect)},
			{"a header length", header_length, COUNT(header_length)},
			{"a modulo", modulo, COUNT(modulo)},
			{"a division by zero", by_zero, COUNT(by_zero)},
			{"a store past memory", past_memory, COUNT(past_memory)},
			{"a load of memory never stored", unwritten, COUNT(unwritten)},
			{"an unknown instruction", unknown, COUNT(unknown)},
	};
	for(size_t i = 0; i < COUNT(programs); i++) {
		errno = 0;
		long got = install(programs[i].insns, programs[i].len, 0);
		if(got != -1 || errno != EINVAL) {
			printf("%s: a program with %s was not refused\n", check_name, programs[i].what);
			check_failed(__FILE__, __LINE__, "the program was refused with EINVAL", errno);
		}
	}
	CHECK(mode() == 0);
}

// The chain may hold 32,768 instructions as Linux counts them: those of
// each filter's conversion to eBPF, four more than a filter of jumps and a
// RET has, and four more for each filter before the newest. Seven of the
// longest filters and one of 4,036 instructions fill it, and no other
// fits.
static void length(void) {
	CHECK(set_no_new_privs() == 0);
	static struct insn longest[MAX_INSTRUCTIONS];
	for(size_t i = 0; i + 1 < COUNT(longest); i++)
		longest[i] = (struct insn)STMT(JA, 0);
	longest[COUNT(longest) - 1] = (struct insn)STMT(RET_K, RET_ALLOW);
	for(int i = 0; i < 7; i++)
		CHECK(install(longest, COUNT(longest), 0) == 0);
	CHECK_ERROR(install(longest, COUNT(longest), 0), ENOMEM);
	size_t room = MAX_INSTRUCTIONS_PER_PATH - 7 * (MAX_INSTRUCTIONS + 4) - 7 * 4 - 4;
	CHECK(room == 4036);
	CHECK(install(longest + COUNT(longest) - room, room, 0) == 0);
	CHECK_ERROR(install(longest + COUNT(longest) - 1, 1, 0), ENOMEM);
	CHECK_STATUS("Seccomp_filters:\t8");
	CHECK(getpid() > 0);
}

int main(int argc, char **argv) {
	if(argc > 1 && strcmp(argv[1], "--inherited") == 0)
		return inherited();
	check_start("seccomp");
	self = argv[0];
	struct utsname names;
	on_dukovo = uname(&names) == 0 && strstr(names.version, "Dukovo") != NULL;
	has_proc = access("/proc/self/status", R_OK) == 0;
	printf("seccomp: running on %s, %s /proc\n", on_dukovo ? "Dukovo" : "Linux",
			has_proc ? "with" : "without");
	if(!on_dukovo)
		CHECK(has_proc);
	if(argc > 1 && strcmp(argv[1], "--status") == 0) {
		CHECK(has_proc);
		CHILD(status_lines);
		return check_end();
	}
	// The processes SIGSYS ends leave no core behind.
	struct rlimit no_core = {0, 0};
	setrlimit(RLIMIT_CORE, &no_core);

	RUN(before);
	CHILD(privileges);
	CHILD(errno_action);
	CHILD(errno_zero);
	CHILD(errno_clamped);
	CHILD(allow_and_log);
	CHILD(trace_and_notify);
	CHILD(trap);
	CHILD_KILLED(trap_blocked, SIGSYS);
	CHILD_KILLED(trap_ignored, SIGSYS);
	CHILD_KILLED(trap_default, SIGSYS);
	CHILD_KILLED(kill_process, SIGSYS);
	CHILD_KILLED(unknown_action, SIGSYS);
	CHILD_KILLED(kill_only_thread, SIGSYS);
	CHILD(kill_one_thread);
	CHILD_KILLED(kill_from_thread, SIGSYS);
	CHILD(arguments);
	CHILD(chain);
	CHILD(forwarded);
	CHILD(inheritance);
	CHILD(native_program);
	CHILD(tsync);
	CHILD(tsync_refused);
	CHILD(tsync_strict);
	CHILD(tsync_shared);
	RUN(strict_mode);
	CHILD(refused);
	CHILD(length);
	return check_end();
}
