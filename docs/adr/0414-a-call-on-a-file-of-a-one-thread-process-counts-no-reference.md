<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0414. A call on a file of a one-thread process counts no reference

- Status: Accepted
- Date: 2026-11-08

## Context

CI holds the open and close scaling rows to Linux's time (ADR 0409).
After ADR 0413 the AMD EPYC 9V74 runners' process rows were 0.93, 0.95
and 0.98 of Linux's: eight processes took 1,452 ns per open and close
against Linux's 1,478. The workers share no cache lines, so what is left
is what each call costs.

Each open and close takes 12 locked read-modify-writes. Two are the
directory's reference count. `openat` is a method of the directory
capability, and the CPU's capability cache (ADR 0369) keeps a weak
reference to the object, so that it never holds an object the store let
go of: every call upgrades it to a strong one, a compare-and-swap loop,
and drops it after, a locked decrement. A file's `read` and `write` pay
the same.

Linux pays neither in a process with one thread. `fdget` takes no
reference when the descriptor table is not shared (`files->count` is
one): only the calling thread can close the descriptor, and it is busy
with this call, so the table's reference keeps the file.

Radix's capability space is not quite as private. Besides the process's
own threads, three things can empty a slot in it:

- a revoke of an ancestor capability, held by another process, which
  deletes every descendant: `fork` derives the child's capabilities from
  the parent's, so the benchmark's workers, like every forked program,
  hold capabilities their parent can revoke;
- a thread another process starts in this one (`process.start`), which
  then runs as one of its own;
- the end of the process, which destroys the space only after its last
  thread has exited.

IPC moves a capability out of the sender's space only while the sender
waits in its send, and a thread leaving an endpoint's queue takes its
message back first; giving, sharing and creating put capabilities only
in empty slots.

## Decision

**A call on a file or directory capability of a process with one thread,
with no revoke from a program under way, borrows the object without
counting a reference.** The lookup (`objects::find_holding`), in one
read section with preemption off, reads the store's version, finds the
capability in the CPU's cache at that version, reads that the process
has one live thread and that no program's revoke is under way, and
records the borrowed object in a per-CPU word. The call uses the object
through an `Arc` made from the cache's pointer, uncounted, which only the
system call's own place for what it finds holds (`objects::Lent`). When
the call ends, the word is cleared with interrupts off: if the object is
still there, the `Arc` is forgotten; if not, it was counted meanwhile,
and the `Arc` is dropped.

**The scheduler counts a borrow before its CPU passes a quiescent
state.** In `reschedule`, before a quiescent state is reported or the
CPU switches threads, and in the timer tick before it reports one, the
per-CPU word is read: a borrowed object gets a strong reference and the
word is cleared. A call that sleeps, or is preempted, or only takes a
tick, so holds a real reference from then on, wherever it runs next.

**Whoever else can empty the slot waits for a grace period first.** A
revoke a program asks for (`capability.revoke`) counts itself in a
global count of revokes under way, waits for a grace period
(`rcu::synchronize`), revokes, and counts itself out
(`objects::revoking`). A thread that another process starts in a process
that already has threads is counted among them, then waits for a grace
period before it first runs. The kernel's own revoke at `exec` revokes an
address space's capabilities, never a file's, and is left as it was.

So while a borrow is not counted, its capability stays in its slot, and
the store's reference keeps the object: the thread does not empty its
own slot during its own call, it has no other thread, and anyone who
could empty the slot from outside started after the lookup's read
section and waits for a grace period, which ends only once the
borrowing CPU has passed a quiescent state, by which time the borrow is
counted or over. One that started before it is seen by the lookup: the
grace period's start and each CPU's quiescent state are ordered by the
RCU state's lock, so a lookup whose read section the grace period does
not wait for reads the count of revokes, or of threads, after it moved,
and counts its reference.

`kernel/radix/tla/LightBorrow.tla` models one directory with a
capability in each of two processes, `S` derived from `D0`: the
borrowing thread, which may also close `S` and start a second thread;
two threads of the other process, which close `D0`, revoke it and start
a thread in the first; and that thread, which closes `S`. TLC checks that
nothing uses the object once it is freed and that a borrow not counted
always has its capability in `S`. Without the counting before a
quiescent state, without the revoke's grace period, or without the
start's, both fail, as they do if the lookup borrows with two threads or
during a revoke.

## Consequences

- An open and close takes 10 locked read-modify-writes, from 12, and a
  file's `read` and `write` two fewer each. Counted under QEMU (`ob bench
  --instructions`), an open and close runs 3,649 instructions, from
  3,645, and a 4 KiB read 4,984, from 4,979. Other methods pay for the
  check: 64 random bytes take 2,203, from 2,185, and an endpoint call and
  reply 5,242, from 5,198; a yield and a switch read the per-CPU word
  (637 from 628, 1,004 from 991).
- A revoke a program asks for takes a grace period longer, about a timer
  tick at most, and while one is under way every call counts its
  references. Programs revoke rarely: POSIX never does.
- A thread started from another process into one that has threads takes
  a grace period longer to start. `pthread_create`, `fork` and `exec`
  start threads from inside the process, or into one with none, and do
  not wait.
- The tick and every switch read one per-CPU word more.
- Methods on a borrowed file see `&Found` only, so none can take the
  uncounted `Arc` out of it. The borrow lives only in the place the
  system call gives the lookup (`objects::Lent`), which it ends at the
  one point every way out of the call passes; a place dropped without
  that would forget what it holds, a leak rather than a reference
  dropped twice. Every other lookup's `Found` has nothing to give back,
  and costs what it did. A debug assertion checks that a call never
  releases the object it borrowed.
- A process with two threads or more counts its references as before,
  as Linux does when the table is shared.

## Alternatives considered

- **The cache keeps strong references**, dropped when the version moves.
  A cached object would then outlive its last capability until every
  CPU's cache let it go: a closed pipe's reader would not see the end of
  file, locks would stay held and an unmount would find the file busy,
  unless every close sent other CPUs an interrupt to flush.
- **Only capabilities with no parent**, which no revoke can reach. Every
  capability `fork` copies has one, the benchmark's included, so it would
  save nothing where it matters.
- **Every release of a cached object waits for a grace period.** It
  would cover revokes without making them wait, but `close` would no
  longer let go of a file at once, with the same effects as above.
- **A global SRCU read section around the call.** Calls sleep while
  holding the borrow, and a writer that waits for them could wait on a
  call that waits on it.
