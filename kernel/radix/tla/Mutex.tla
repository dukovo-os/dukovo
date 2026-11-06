\* SPDX-FileCopyrightText: 2026 The Dukovo contributors
\* SPDX-License-Identifier: Apache-2.0 OR MIT

-------------------------------- MODULE Mutex -------------------------------
(***************************************************************************)
(* A model of the kernel's mutex (radix-sync's mutex.rs, and the hooks in  *)
(* kernel/radix/src/sched/wait.rs, ADR 0368): threads take one mutex and   *)
(* let it go, some of them real-time.                                      *)
(*                                                                         *)
(* The mutex is a word: its owner, or none, and the waiters bit. Taking a  *)
(* free mutex whose bit is clear is one compare-and-swap; so is letting go *)
(* of one whose bit is clear. Otherwise a thread spins: it takes the       *)
(* mutex if it finds it without an owner, bit or no bit, and gives up when *)
(* the owner does not run, which the model leaves to chance. Then it       *)
(* queues, with the wait table locked: it takes the mutex if it has no     *)
(* owner (setting the bit if others are queued), or sets the bit and       *)
(* joins the queue, where it keeps the place it had the first time, and    *)
(* sleeps. The owner lets go of a mutex with the bit set with the table    *)
(* locked: to the most urgent waiter (real-time first, then the first to   *)
(* come) outright if it is real-time or has lost the mutex MaxLosses times *)
(* already (a handoff), and otherwise by clearing the owner and waking     *)
(* that waiter to try again (a let-go), when a spinner may take it first.  *)
(* Either way the waiter leaves the queue, the bit stays only if others    *)
(* remain, and the waiter's `blocked_on` is cleared after the word is      *)
(* written; a woken waiter that does not find itself the owner counts a    *)
(* loss and spins again.                                                   *)
(*                                                                         *)
(* MutualExclusion says one thread at a time is in its critical section.   *)
(* BitMeansWaiters says the bit is set exactly while someone is queued,    *)
(* so an owner never lets go with the fast compare-and-swap while a waiter *)
(* sleeps: a lost wake-up. HandoffToRealTime says a real-time waiter that  *)
(* is the most urgent gets the mutex outright, so nobody takes it in       *)
(* between (the scheduler's property 8). BoundedLosses says a fair waiter  *)
(* loses the mutex at most MaxLosses times before it is handed over.       *)
(* EveryRoundEnds says every thread gets the mutex every time it asks:     *)
(* no thread sleeps for ever. A let-go that keeps the bit clear while      *)
(* others are queued strands them; one that hands over without clearing   *)
(* `blocked_on` leaves the new owner asleep.                               *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    Threads,    \* the threads that take the mutex
    RealTime,   \* those of them that are real-time
    Rounds,     \* how many times each thread takes it
    MaxLosses,  \* LOSSES_BEFORE_HANDOFF
    None        \* no owner

ASSUME RealTime \subseteq Threads /\ None \notin Threads

VARIABLES
    owner,      \* the mutex's owner, or None
    bit,        \* the waiters bit
    queued,     \* the threads in the wait table for the mutex
    seq,        \* each thread's place in the queue, kept when it queues again; 0 before
    next,       \* the next place to give
    blocked,    \* each thread's `blocked_on`: set while it is queued
    losses,     \* times each thread woke to find the mutex taken
    pc,         \* where each thread is
    round       \* how many times each thread has taken the mutex

vars == <<owner, bit, queued, seq, next, blocked, losses, pc, round>>

Steps == {"take", "spin", "queue", "sleep", "check", "critical", "release", "done"}

TypeOK ==
    /\ owner \in Threads \union {None}
    /\ bit \in BOOLEAN
    /\ queued \subseteq Threads
    /\ seq \in [Threads -> Nat]
    /\ next \in Nat \ {0}
    /\ blocked \in [Threads -> BOOLEAN]
    /\ losses \in [Threads -> 0..MaxLosses]
    /\ pc \in [Threads -> Steps]
    /\ round \in [Threads -> 0..Rounds]

Init ==
    /\ owner = None
    /\ bit = FALSE
    /\ queued = {}
    /\ seq = [t \in Threads |-> 0]
    /\ next = 1
    /\ blocked = [t \in Threads |-> FALSE]
    /\ losses = [t \in Threads |-> 0]
    /\ pc = [t \in Threads |-> "take"]
    /\ round = [t \in Threads |-> 0]

\* The most urgent of the queued threads: real-time first, then the
\* lowest place (`Table::best`).
Urgent(t, u) ==
    \/ t \in RealTime /\ u \notin RealTime
    \/ (t \in RealTime) = (u \in RealTime) /\ seq[t] < seq[u]
Best == CHOOSE t \in queued : \A u \in queued \ {t} : Urgent(t, u)

----------------------------------------------------------------------------
(* A thread taking the mutex (`Mutex::lock`, then `mutex_wait`).           *)

\* The fast path: one compare-and-swap of a word that is all zero.
Take(t) ==
    /\ pc[t] = "take"
    /\ IF owner = None /\ ~bit
       THEN /\ owner' = t
            /\ pc' = [pc EXCEPT ![t] = "critical"]
       ELSE /\ pc' = [pc EXCEPT ![t] = "spin"]
            /\ UNCHANGED owner
    /\ UNCHANGED <<bit, queued, seq, next, blocked, losses, round>>

\* `spin_while_owner_runs`: take a mutex with no owner, keeping the bit;
\* or give up, as when the owner does not run or the spin ends.
Spin(t) ==
    /\ pc[t] = "spin"
    /\ \/ /\ owner = None
          /\ owner' = t
          /\ pc' = [pc EXCEPT ![t] = "critical"]
       \/ /\ pc' = [pc EXCEPT ![t] = "queue"]
          /\ UNCHANGED owner
    /\ UNCHANGED <<bit, queued, seq, next, blocked, losses, round>>

\* `queue`, with the table locked.
Queue(t) ==
    /\ pc[t] = "queue"
    /\ IF owner = None
       THEN /\ owner' = t
            /\ bit' = (queued # {})
            /\ pc' = [pc EXCEPT ![t] = "critical"]
            /\ UNCHANGED <<queued, seq, next, blocked>>
       ELSE /\ bit' = TRUE
            /\ queued' = queued \union {t}
            /\ IF seq[t] = 0
               THEN /\ seq' = [seq EXCEPT ![t] = next]
                    /\ next' = next + 1
               ELSE UNCHANGED <<seq, next>>
            /\ blocked' = [blocked EXCEPT ![t] = TRUE]
            /\ pc' = [pc EXCEPT ![t] = "sleep"]
            /\ UNCHANGED owner
    /\ UNCHANGED <<losses, round>>

\* Asleep until `blocked_on` is cleared.
Sleep(t) ==
    /\ pc[t] = "sleep"
    /\ ~blocked[t]
    /\ pc' = [pc EXCEPT ![t] = "check"]
    /\ UNCHANGED <<owner, bit, queued, seq, next, blocked, losses, round>>

\* Handed the mutex, or let go to the waiters: a loss, and spin again.
Check(t) ==
    /\ pc[t] = "check"
    /\ IF owner = t
       THEN /\ pc' = [pc EXCEPT ![t] = "critical"]
            /\ UNCHANGED losses
       ELSE /\ losses' = [losses EXCEPT ![t] = IF @ < MaxLosses THEN @ + 1 ELSE @]
            /\ pc' = [pc EXCEPT ![t] = "spin"]
    /\ UNCHANGED <<owner, bit, queued, seq, next, blocked, round>>

----------------------------------------------------------------------------
(* The owner.                                                              *)

Critical(t) ==
    /\ pc[t] = "critical"
    /\ pc' = [pc EXCEPT ![t] = "release"]
    /\ UNCHANGED <<owner, bit, queued, seq, next, blocked, losses, round>>

\* The guard's drop: the fast compare-and-swap if the bit is clear, else
\* `mutex_wake` with the table locked. The thread's next round starts
\* afresh: a new place, no losses.
Release(t) ==
    /\ pc[t] = "release"
    /\ IF ~bit
       THEN /\ owner' = None
            /\ UNCHANGED <<bit, queued, blocked>>
       ELSE LET w == Best
                rest == queued \ {w}
            IN  /\ queued' = rest
                /\ bit' = (rest # {})
                /\ blocked' = [blocked EXCEPT ![w] = FALSE]
                /\ owner' = IF w \in RealTime \/ losses[w] = MaxLosses
                            THEN w                  \* `hand_over`
                            ELSE None               \* `let_go_to_waiters`
    /\ round' = [round EXCEPT ![t] = @ + 1]
    /\ seq' = [seq EXCEPT ![t] = 0]
    /\ losses' = [losses EXCEPT ![t] = 0]
    /\ pc' = [pc EXCEPT ![t] = IF round[t] + 1 = Rounds THEN "done" ELSE "take"]
    /\ UNCHANGED next

Thread(t) ==
    \/ Take(t) \/ Spin(t) \/ Queue(t) \/ Sleep(t) \/ Check(t)
    \/ Critical(t) \/ Release(t)

Done == \A t \in Threads : pc[t] = "done" /\ UNCHANGED vars

Next == (\E t \in Threads : Thread(t)) \/ Done

\* Each thread keeps taking steps while it can; spinning gives up in the
\* end (weak fairness on the thread's steps covers both).
Spec == Init /\ [][Next]_vars /\ \A t \in Threads : WF_vars(Thread(t))

----------------------------------------------------------------------------
(* Properties.                                                             *)

MutualExclusion ==
    \A t \in Threads : pc[t] \in {"critical", "release"} => owner = t

BitMeansWaiters == bit = (queued # {})

\* Only a queued thread sleeps, and it sleeps until it is dequeued.
SleepersAreQueued ==
    \A t \in Threads : blocked[t] <=> t \in queued

\* An owner that is not in its critical section was handed the mutex
\* while it slept, and is on its way in.
OwnerIsComing ==
    owner # None => pc[owner] \in {"critical", "release", "sleep", "check"}

\* A real-time thread that the queue would serve first is never passed
\* over: when the owner lets go, it gets the mutex outright.
HandoffToRealTime ==
    \A t \in Threads :
        (pc[t] = "check" /\ t \in RealTime) => owner = t

BoundedLosses == \A t \in Threads : losses[t] <= MaxLosses

EveryRoundEnds == <>(\A t \in Threads : pc[t] = "done")

=============================================================================
