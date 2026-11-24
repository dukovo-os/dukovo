<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0657. Each thread keeps a cache of its freed blocks

- Status: Accepted
- Date: 2026-11-24

## Context

After the work of ADR 0650 to 0655, PostgreSQL, PHP-FPM behind nginx
and Valkey pipelined were still 1.10 to 1.28 times Linux's time on the
OptiPlex. Splitting each pass's CPU time between the benchmark's client
and the machine (serverbench's `BENCHCLIENT` lines) put the difference
in the servers' own user time: 17% more a PostgreSQL select, 47% more a
TPC-B transaction, 21% more an nginx and PHP-FPM request, 24% more a
pipelined Valkey GET, while PHP's compute row, which allocates through
PHP's own memory manager, was within 3%.

serverbench's `server_malloc_one` frees the oldest of 1,024 small blocks
and allocates one of 16 to 512 bytes, and `server_malloc_two` does the
same on two threads at once. On the OptiPlex a round took 65.3 ns on
Dukovo against glibc's 19.7 ns, and 290.2 ns on two threads against
21.9 ns. mlibc's malloc is frigg's slab pool: every malloc and every
free takes the lock of the block's size, one lock all threads share,
and mlibc's lock sleeps in the kernel as soon as it finds another thread
holding it; the pool also moves a slab in and out of a red-black tree
whenever the slab fills or empties. glibc keeps a cache of freed blocks
for each thread (tcache), which takes no lock and no atomic operation.

## Decision

- **Each thread keeps freed blocks of 16 bytes to 4 KiB, and malloc
  hands them out again first**: mlibc patch 0141. A cache holds 16
  blocks of each size up to 1 KiB and 4 of 2 KiB and of 4 KiB, at most
  56 KiB a thread. The sizes are the pool's buckets, so a cached block is
  one the pool handed out and can always take back, from any thread.
- **The pool is used half a cache at a time.** A cache with no block of
  a size takes 8 (2 of the largest sizes) with one hold of the size's
  lock, and a full one gives back its older half the same way
  (`slab_pool::allocate_batch` and `free_batch`), keeping the half freed
  last, likelier to be in the processor's cache.
- **Hardened as glibc hardens tcache.** A cached block's first word
  links it to the next of its size, mangled with the word's own address
  and a secret of the process's, which glibc does not have; malloc
  checks that each link it follows leads to a block of the size, or to
  nothing after the last, and ends the process with SIGABRT if not. The
  second word holds a key of the process's while the block is cached: a
  free that finds the key looks for the block in the cache and ends the
  process on a double free. malloc clears the key as it hands a block
  out. The secret and the key come from getentropy once a process.
- **The cache is in TLS, initial-exec**, as the C library is always
  loaded with the program: a load from the thread pointer, and no change
  to the TCB, whose layout the assembly of the other architectures
  depends on. thread_exit gives a thread's blocks back to the pool after
  the thread's destructors have run, and its later frees go to the pool.
- `posix_memalign`, `aligned_alloc` and `memalign` take a cached block
  when the alignment is at most the block's size: every block of a
  bucket lies at a multiple of its size. A malloc of 8 bytes or fewer
  takes a 16-byte block, as an 8-byte one has no room for the key
  (glibc's smallest chunk holds 24 bytes). `MLIBC_DEBUG_MALLOC` turns
  the cache off, as only the pool's path logs.

## Alternatives

- **Larger caches.** With the patched C library built for Linux and run
  on the same host, two threads took 30.5 ns a round with 16 blocks of a
  size, 23.7 ns with 32 and 18 ns with 64 (glibc 21 ns). Each refill
  writes a link into every block it takes, so a larger batch touches more
  pages in every process for every size it uses, which the idle-memory
  row counts; the single-thread rows, where the servers are, gained only
  1 to 2 ns.
- **No batches**, a block at a time between the cache and the pool, as
  glibc's tcache does with its arenas. In a host test of the cache with
  frigg's pool, two threads took 31.8 ns a round where batches took
  22.6 ns: a cache that is empty or full goes to the pool on every
  operation until it is neither, and glibc's arenas, one for each thread,
  rarely contend where frigg's pool always can.
- **The cache in the TCB.** As fast as TLS, but adding a field moves the
  end of the TCB, from which aarch64 and RISC-V address the fields their
  assembly uses, and those ports would have to change the offsets.
- **A lock that spins before it sleeps**, for every lock in mlibc. It
  would make the pool cheaper to contend for, but not as cheap as no
  lock, and it changes every other lock's behaviour as well.

## Consequences

- serverbench's rows, Dukovo before and after against Linux (lab run
  20261124-230701-2bac): `server_malloc_one` 65.3 to 15.6 ns on the
  OptiPlex (Linux 19.1 ns) and 25.3 to 5.1 ns under QEMU (Linux 7.9 ns);
  `server_malloc_two` 290.2 to 24.1 ns on the OptiPlex (Linux 21.9 ns)
  and 125.1 to 9.9 ns under QEMU (Linux 9.3 ns). The patched mlibc, built
  for Linux and run on the container host, gave the same picture: 10 ns
  a round on one thread, from 52 to 56 ns, and 30.5 ns on two, from 413
  to 435 ns.
- The server rows did not move beyond their run-to-run spread: on the
  OptiPlex PostgreSQL's TPC-B is 1.24 times Linux's time (1.27 to 1.31
  before), its select 1.11 (1.12 to 1.14), PHP-FPM 1.14 (1.10 to 1.14)
  and pipelined Valkey GETs 1.19 (1.17 to 1.23). The servers' extra user
  time is mostly elsewhere than malloc. The idle machine uses 0.3 MiB
  more on the OptiPlex and 1.3 MiB more under QEMU, within the
  idle-memory row's budget.
- A double free of a block in the thread's cache, and a write through a
  dangling pointer to a cached block's link, end the process with
  SIGABRT where they corrupted the pool before; a double free of a block
  in another thread's cache, or one the cache has given back, is not
  found, as with glibc.
- A fork keeps the forking thread's cache in the child; the blocks in
  other threads' caches stay allocated in the child, at most 56 KiB for
  each such thread, as glibc's tcache leaves them.
- A signal handler that calls malloc while its thread is in malloc or
  free can now damage the cache's lists, where it could deadlock on the
  pool's lock before: malloc is not async-signal-safe, here or in glibc.
- The posix boot test `library` runs four threads that allocate, free
  and hand each other blocks of every size, checking that no block
  changed while it was theirs; checks that 300 threads that fill their
  caches and exit leave no more memory behind than 300 that allocate
  nothing; and checks that a double free and a flipped bit in a freed
  block's link end a child with SIGABRT.
- Every port rebuilds once, as for any change to mlibc. There is no ABI
  change.
