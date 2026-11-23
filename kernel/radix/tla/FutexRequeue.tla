\* SPDX-FileCopyrightText: 2026 The Dukovo contributors
\* SPDX-License-Identifier: Apache-2.0 OR MIT

--------------------------- MODULE FutexRequeue ---------------------------
(***************************************************************************)
(* A model of futex requeues between two words, A and B                    *)
(* (kernel/radix/src/user/memory/futex.rs: `requeue`, `move_waiters`,      *)
(* `Pair::lock`, and `leave`, which a waiter that stops waiting calls),    *)
(* with the scheduler's `block` and `wake` beneath them, as Futex.tla has  *)
(* them. It is a module of its own because Futex.tla models one word, and  *)
(* a second list and a requeuer would bury what it checks.                 *)
(*                                                                         *)
(* Each word has a list of waiters, in arrival order. A waiter has a       *)
(* variable the model keeps apart on purpose: its `bucket`, the name of    *)
(* the list it is in, which is read without any lock and changed only with *)
(* the list it names locked. Waiters start in the list of A (or of B, for  *)
(* those in `StartB`), with their flag clear, the way `wait` leaves them.  *)
(* The word's value, which Futex.tla checks, is left out: wakers and       *)
(* requeuers start once every waiter has queued, which is what the word's  *)
(* compare guarantees.                                                     *)
(*                                                                         *)
(* A waker of a word takes up to `Count` waiters off the front of that     *)
(* word's list, setting their flags, in one step with the list locked, and *)
(* then wakes their threads, one step each. A requeuer takes both locks in *)
(* one step: it takes up to `Wakes` waiters off the front of A's list as   *)
(* woken, then moves up to `Move` of the next ones to the end of B's list, *)
(* setting each one's `bucket` to B in the same step. It may be out of     *)
(* memory for a moved waiter, which is then woken instead of moved: it     *)
(* leaves A's list with its flag set and `bucket` unchanged, and counts as *)
(* moved. The model chooses, for each moved waiter, whether memory is      *)
(* short.                                                                  *)
(*                                                                         *)
(* A waiter that a signal interrupts and that finds its flag clear calls   *)
(* `leave`: it reads its `bucket` (one step), locks that list and checks   *)
(* that `bucket` still names it (one step; if it does not, a requeue moved *)
(* it in between, and it reads again). With the lock held and `bucket`     *)
(* still right, the waiter is in that list and leaves it, or is in none,   *)
(* as a waker took it off first, which counts as its wake-up.              *)
(*                                                                         *)
(* NoLostWakeup says a waiter never sleeps with its flag set unless a      *)
(* waker or requeuer has yet to wake it. WakesCount says a wait ends as    *)
(* woken exactly when a waker or requeuer took it off a list, so no wake   *)
(* is spent on a waiter that reports a signal, and none is lost to one     *)
(* that was moved. AtMostOneList says a waiter is in at most one list, and *)
(* BucketIsCurrent that a waiter in a list has `bucket` naming it and its  *)
(* flag clear, and that one whose wait has ended is in none. EveryWaitEnds *)
(* says every wait ends, which holds because the wakers of B wake as many  *)
(* as there are waiters.                                                   *)
(*                                                                         *)
(* The check fails if `leave` does not look at `bucket` again after it     *)
(* has locked the list: that is, if the `Lock` step below has `IF FALSE`   *)
(* in place of `IF bucket[w] # seen[w]`. TLC then finds, in nine states,   *)
(* a waiter that reads `bucket` = A and a requeue that moves it to B;      *)
(* the waiter then locks A, does not find itself there, and ends its       *)
(* wait as woken while it is in B's list with its flag clear. WakesCount   *)
(* fails, since no waker took it off a list, and a waker of B would        *)
(* later take a thread that has returned.                                  *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    Waiters,    \* threads that wait
    StartB,     \* the waiters that begin in B's list; the others, in A's
    WakersA,    \* threads that wake waiters of A
    WakersB,    \* threads that wake waiters of B
    Requeuers,  \* threads that requeue from A to B
    Count,      \* how many each waker wakes at most
    Wakes,      \* how many each requeuer wakes at most
    Move,       \* how many each requeuer moves at most
    Signalled   \* waiters that may be interrupted

ASSUME StartB \subseteq Waiters /\ Signalled \subseteq Waiters

Words == {"A", "B"}
Actors == WakersA \cup WakersB \cup Requeuers

VARIABLES
    lists,      \* each word's waiters, in arrival order
    bucket,     \* each waiter's `futex_bucket`: the list it is in
    flag,       \* each waiter's futex_woken
    taken,      \* whether a waker or requeuer took the waiter off a list
    intr,       \* the waiter was interrupted
    asleep,     \* the scheduler has the waiter blocked
    pending,    \* the scheduler holds a wake for its next block
    result,     \* how each wait ended
    seen,       \* the list `leave` read from `bucket`
    wpc,        \* where each waiter is
    apc,        \* where each waker and requeuer is
    batch       \* the waiters each of them has taken and not yet woken

vars == <<lists, bucket, flag, taken, intr, asleep, pending, result, seen, wpc, apc, batch>>

WaiterSteps == {"queue", "loop", "block", "asleep", "read", "lock", "done"}
ActorSteps == {"go", "wake", "done"}
Results == {"none", "woken", "interrupted"}

InList(w, l) == \E i \in 1..Len(lists[l]) : lists[l][i] = w
Listed(w) == \E l \in Words : InList(w, l)

TypeOK ==
    /\ lists \in [Words -> Seq(Waiters)]
    /\ bucket \in [Waiters -> Words \cup {"none"}]
    /\ flag \in [Waiters -> BOOLEAN]
    /\ taken \in [Waiters -> BOOLEAN]
    /\ intr \in [Waiters -> BOOLEAN]
    /\ asleep \in [Waiters -> BOOLEAN]
    /\ pending \in [Waiters -> BOOLEAN]
    /\ result \in [Waiters -> Results]
    /\ seen \in [Waiters -> Words \cup {"none"}]
    /\ wpc \in [Waiters -> WaiterSteps]
    /\ apc \in [Actors -> ActorSteps]
    /\ batch \in [Actors -> SUBSET Waiters]

Init ==
    /\ lists = [l \in Words |-> <<>>]
    /\ bucket = [w \in Waiters |-> "none"]
    /\ flag = [w \in Waiters |-> FALSE]
    /\ taken = [w \in Waiters |-> FALSE]
    /\ intr = [w \in Waiters |-> FALSE]
    /\ asleep = [w \in Waiters |-> FALSE]
    /\ pending = [w \in Waiters |-> FALSE]
    /\ result = [w \in Waiters |-> "none"]
    /\ seen = [w \in Waiters |-> "none"]
    /\ wpc = [w \in Waiters |-> "queue"]
    /\ apc = [k \in Actors |-> "go"]
    /\ batch = [k \in Actors |-> {}]

\* The scheduler's wake: a blocked thread runs, any other holds the wake
\* for its next block.
SchedWake(w) ==
    IF asleep[w]
    THEN /\ asleep' = [asleep EXCEPT ![w] = FALSE]
         /\ UNCHANGED pending
    ELSE /\ pending' = [pending EXCEPT ![w] = TRUE]
         /\ UNCHANGED asleep

----------------------------------------------------------------------------
(* A waiter (`wait`, then `leave`).                                        *)

\* `wait`'s critical section: queued with the flag clear, `bucket` set.
Queue(w) ==
    /\ wpc[w] = "queue"
    /\ LET l == IF w \in StartB THEN "B" ELSE "A" IN
       /\ lists' = [lists EXCEPT ![l] = Append(@, w)]
       /\ bucket' = [bucket EXCEPT ![w] = l]
    /\ flag' = [flag EXCEPT ![w] = FALSE]
    /\ wpc' = [wpc EXCEPT ![w] = "loop"]
    /\ UNCHANGED <<taken, intr, asleep, pending, result, seen, apc, batch>>

\* `while !done() { block }`, and what follows it.
Look(w) ==
    /\ wpc[w] = "loop"
    /\ IF flag[w]
       THEN /\ result' = [result EXCEPT ![w] = "woken"]
            /\ wpc' = [wpc EXCEPT ![w] = "done"]
       ELSE /\ wpc' = [wpc EXCEPT ![w] = IF intr[w] THEN "read" ELSE "block"]
            /\ UNCHANGED result
    /\ UNCHANGED <<lists, bucket, flag, taken, intr, asleep, pending, seen, apc, batch>>

Block(w) ==
    /\ wpc[w] = "block"
    /\ IF pending[w]
       THEN /\ pending' = [pending EXCEPT ![w] = FALSE]
            /\ wpc' = [wpc EXCEPT ![w] = "loop"]
            /\ UNCHANGED asleep
       ELSE /\ asleep' = [asleep EXCEPT ![w] = TRUE]
            /\ wpc' = [wpc EXCEPT ![w] = "asleep"]
            /\ UNCHANGED pending
    /\ UNCHANGED <<lists, bucket, flag, taken, intr, result, seen, apc, batch>>

Wake(w) ==
    /\ wpc[w] = "asleep" /\ ~asleep[w]
    /\ wpc' = [wpc EXCEPT ![w] = "loop"]
    /\ UNCHANGED <<lists, bucket, flag, taken, intr, asleep, pending, result, seen, apc, batch>>

\* `leave`'s read of `futex_bucket`, with no lock held.
Read(w) ==
    /\ wpc[w] = "read"
    /\ seen' = [seen EXCEPT ![w] = bucket[w]]
    /\ wpc' = [wpc EXCEPT ![w] = "lock"]
    /\ UNCHANGED <<lists, bucket, flag, taken, intr, asleep, pending, result, apc, batch>>

\* `leave`'s critical section: the list `seen` names is locked, and
\* `bucket` is read again. If a requeue moved the waiter, `bucket` changed,
\* and the waiter reads it again; if not, the waiter is in the list or a
\* waker took it off, which is its wake-up.
Lock(w) ==
    /\ wpc[w] = "lock"
    /\ IF bucket[w] # seen[w]
       THEN /\ wpc' = [wpc EXCEPT ![w] = "read"]
            /\ UNCHANGED <<lists, result>>
       ELSE /\ IF InList(w, seen[w])
               THEN /\ lists' = [lists EXCEPT ![seen[w]] =
                                    SelectSeq(@, LAMBDA x : x # w)]
                    /\ result' = [result EXCEPT ![w] = "interrupted"]
               ELSE /\ result' = [result EXCEPT ![w] = "woken"]
                    /\ UNCHANGED lists
            /\ wpc' = [wpc EXCEPT ![w] = "done"]
    /\ UNCHANGED <<bucket, flag, taken, intr, asleep, pending, seen, apc, batch>>

Waiter(w) == Queue(w) \/ Look(w) \/ Block(w) \/ Wake(w) \/ Read(w) \/ Lock(w)

----------------------------------------------------------------------------
(* Wakers and requeuers start once every waiter has queued; wakers of B    *)
(* start once the requeuers have taken their step, as a waiter moved to B  *)
(* after the last wake of B would wait for a wake the program never sends. *)

Started == \A w \in Waiters : wpc[w] # "queue"

Min(a, b) == IF a <= b THEN a ELSE b

\* A waker's critical section: up to `Count` waiters off the front of the
\* list of its word, flags set; then it wakes their threads.
Take(k) ==
    /\ apc[k] = "go" /\ k \notin Requeuers /\ Started
    /\ k \in WakersB => \A q \in Requeuers : apc[q] # "go"
    /\ LET l == IF k \in WakersA THEN "A" ELSE "B"
           n == Min(Len(lists[l]), Count)
           out == {lists[l][i] : i \in 1..n}
       IN /\ lists' = [lists EXCEPT ![l] = SubSeq(@, n + 1, Len(@))]
          /\ flag' = [w \in Waiters |-> IF w \in out THEN TRUE ELSE flag[w]]
          /\ taken' = [w \in Waiters |-> IF w \in out THEN TRUE ELSE taken[w]]
          /\ batch' = [batch EXCEPT ![k] = out]
          /\ apc' = [apc EXCEPT ![k] = IF n = 0 THEN "done" ELSE "wake"]
    /\ UNCHANGED <<bucket, intr, asleep, pending, result, seen, wpc>>

\* A requeuer's critical section, with both lists locked: up to `Wakes`
\* waiters off the front of A's list, woken; up to `Move` of the next ones
\* moved to the end of B's list, `bucket` set in the same step. A moved
\* waiter that memory is short for is woken instead: off A's list with its
\* flag set, `bucket` as it was.
Requeue(k) ==
    /\ apc[k] = "go" /\ k \in Requeuers /\ Started
    /\ LET a == lists["A"]
           n == Min(Len(a), Wakes)
           m == Min(Len(a) - n, Move)
           woken == {a[i] : i \in 1..n}
           moving == SubSeq(a, n + 1, n + m)
           movingSet == {moving[i] : i \in 1..m}
       IN \E short \in SUBSET movingSet :
          LET out == woken \cup short
              kept == SelectSeq(moving, LAMBDA x : x \notin short)
          IN /\ lists' = [l \in Words |->
                            IF l = "A" THEN SubSeq(a, n + m + 1, Len(a))
                            ELSE lists["B"] \o kept]
             /\ bucket' = [w \in Waiters |->
                            IF w \in movingSet /\ w \notin short THEN "B" ELSE bucket[w]]
             /\ flag' = [w \in Waiters |-> IF w \in out THEN TRUE ELSE flag[w]]
             /\ taken' = [w \in Waiters |-> IF w \in out THEN TRUE ELSE taken[w]]
             /\ batch' = [batch EXCEPT ![k] = out]
             /\ apc' = [apc EXCEPT ![k] = IF out = {} THEN "done" ELSE "wake"]
    /\ UNCHANGED <<intr, asleep, pending, result, seen, wpc>>

WakeOne(k) ==
    /\ apc[k] = "wake"
    /\ \E w \in batch[k] :
          /\ SchedWake(w)
          /\ batch' = [batch EXCEPT ![k] = batch[k] \ {w}]
          /\ apc' = [apc EXCEPT ![k] = IF batch[k] = {w} THEN "done" ELSE "wake"]
    /\ UNCHANGED <<lists, bucket, flag, taken, intr, result, seen, wpc>>

Actor(k) == Take(k) \/ Requeue(k) \/ WakeOne(k)

----------------------------------------------------------------------------
(* A signal, a stop or a deadline: the flag, then the scheduler's wake.    *)

Interrupt(w) ==
    /\ w \in Signalled /\ ~intr[w] /\ wpc[w] # "done"
    /\ intr' = [intr EXCEPT ![w] = TRUE]
    /\ SchedWake(w)
    /\ UNCHANGED <<lists, bucket, flag, taken, result, seen, wpc, apc, batch>>

----------------------------------------------------------------------------

Done == (\A w \in Waiters : wpc[w] = "done") /\ (\A k \in Actors : apc[k] = "done")

Next ==
    \/ \E w \in Waiters : Waiter(w) \/ Interrupt(w)
    \/ \E k \in Actors : Actor(k)
    \/ Done /\ UNCHANGED vars

Spec ==
    /\ Init /\ [][Next]_vars
    /\ \A w \in Waiters : WF_vars(Waiter(w))
    /\ \A k \in Actors : WF_vars(Actor(k))

\* Asleep with the flag set, a waiter is in the batch of whoever is still to
\* wake it.
NoLostWakeup ==
    \A w \in Waiters :
        (asleep[w] /\ flag[w]) => \E k \in Actors : w \in batch[k]

\* A wait ends as woken exactly when a waker or requeuer took it off a list.
WakesCount ==
    \A w \in Waiters :
        /\ result[w] = "woken" => taken[w]
        /\ result[w] = "interrupted" => ~taken[w]

\* A waiter is in at most one list, once.
AtMostOneList ==
    \A w \in Waiters :
        /\ Cardinality({l \in Words : InList(w, l)}) <= 1
        /\ \A l \in Words : \A i, j \in 1..Len(lists[l]) :
              (i # j /\ lists[l][i] = w) => lists[l][j] # w

\* A waiter in a list has `bucket` naming it and its flag clear, and one
\* that has ended its wait is in none.
BucketIsCurrent ==
    \A w \in Waiters :
        /\ \A l \in Words : InList(w, l) => (bucket[w] = l /\ ~flag[w] /\ ~taken[w])
        /\ wpc[w] = "done" => ~Listed(w)

\* Every wait ends.
EveryWaitEnds == \A w \in Waiters : <>(wpc[w] = "done")

=============================================================================
