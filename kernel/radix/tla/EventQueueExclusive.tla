\* SPDX-FileCopyrightText: 2026 The Dukovo contributors
\* SPDX-License-Identifier: Apache-2.0 OR MIT

-------------------------- MODULE EventQueueExclusive --------------------------
(***************************************************************************)
(* A model of exclusive watches, which epoll's EPOLLEXCLUSIVE asks for     *)
(* (kernel/radix/src/user/posix/poll.rs: `Polled::changed_by_rmw`, and     *)
(* kernel/radix/src/user/queue.rs: `changed`). It is a module of its own   *)
(* because EventQueue.tla models the waits of one queue, with a watch for  *)
(* each object, and its state would double for watches that share one      *)
(* object; what it checks of a queue's ring, count and wake-ups, this      *)
(* module takes as it has them.                                            *)
(*                                                                         *)
(* One description is watched by several queues, each with one watch of    *)
(* it, kept in `Order`, the order of the description's list of watches.    *)
(* The watches in `Exclusive` are exclusive. A change to the description   *)
(* makes it ready for every watch (`dirty`) and then walks the list: it    *)
(* queues each watch that is not exclusive. For the exclusive ones, it     *)
(* queues each in turn, until `changed` returns true for one: that is, the *)
(* watch is on its queue's ring, by this call or before it, and a thread   *)
(* was counted waiting on the queue when the call read the count (after    *)
(* it woke the waiters, if it had put the watch on the ring). From then on *)
(* the walk leaves the exclusive watches alone, so that one change wakes   *)
(* one queue's waiters and not all of them, as Linux does.                 *)
(*                                                                         *)
(* A queue has one thread that may wait on it: it counts itself, sleeps    *)
(* while the ring is empty, and takes the watch off the ring, clears its   *)
(* queued mark and looks at the description, as in EventQueue.tla, and     *)
(* reports what it finds once `Reports` times at most. Whether a queue's   *)
(* thread waits at all is the model's choice: a thread that is not waiting *)
(* is one that waits on another queue, or later. Each critical section and *)
(* each atomic access is one step, in the order sequential consistency     *)
(* gives them.                                                             *)
(*                                                                         *)
(* NoLostWakeup says a thread never sleeps while its queue's ring holds    *)
(* the watch, unless the walk is still to wake it. WalkQueues says what a  *)
(* finished walk did: every watch that is not exclusive is queued, no      *)
(* exclusive watch after the one that stopped it is queued by it, and the  *)
(* one that stopped it, and every exclusive watch before it, is queued;    *)
(* a walk that nothing stopped queued them all. NoQueueAfterStop says the  *)
(* second part at every step of the walk. SomeThreadReports says that,     *)
(* with a thread waiting on any queue, the change reaches one of them: no  *)
(* queue that has a thread waiting is left without it, unless another      *)
(* thread's report covers it.                                              *)
(*                                                                         *)
(* SomeThreadReports fails if the walk stops at an exclusive watch it has  *)
(* queued whether or not a thread was counted (the `counted` test left out *)
(* of `Read`'s `stop'`): the first queue has no thread waiting, takes the  *)
(* stop, and the second queue's thread sleeps with its watch never queued. *)
(* TLC finds that as a state where the changer is done and the thread is   *)
(* asleep for good.                                                        *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    Order,      \* the queues, in the order of the description's watches
    Exclusive,  \* the queues whose watch is exclusive
    Changes,    \* how many times the description changes
    Reports     \* how many times a thread reports a change, at most

Queues == {Order[i] : i \in 1..Len(Order)}

\* The orders `ob test tla` checks, as a configuration file cannot write a
\* sequence.
TwoQueues == <<"q1", "q2">>
ThreeQueues == <<"q1", "q2", "q3">>

ASSUME Exclusive \subseteq Queues /\ Changes \in Nat \ {0} /\ Reports \in Nat \ {0}
ASSUME \A i, j \in 1..Len(Order) : i # j => Order[i] # Order[j]

VARIABLES
    dirty,      \* each watch's description changed since a look last saw it
    marks,      \* each watch is marked queued
    onRing,     \* each watch is on its queue's ring
    counted,    \* a thread is counted in the queue's `WaitCount`
    inTable,    \* the queue's thread is queued in the wait table
    asleep,     \* the scheduler has the queue's thread blocked
    pending,    \* the scheduler holds a wake for its next block
    wpc,        \* where each queue's thread is
    reports,    \* how many times each queue's thread has reported
    eager,      \* the queues whose thread waits
    cpc,        \* where the changer is
    idx,        \* the watch the walk is at, as a place in `Order`
    made,       \* how many changes the changer has made
    stop,       \* the place in `Order` that stopped the walk, or 0
    delivered   \* the watches this change's walk queued, or found queued

vars == <<dirty, marks, onRing, counted, inTable, asleep, pending, wpc, reports, eager, cpc, idx,
          made, stop, delivered>>

WaiterSteps == {"none", "peek", "pop", "clear", "look", "count", "wait", "block", "asleep",
                "woken", "uncount", "done"}
ChangerSteps == {"change", "visit", "mark", "push", "check", "wake", "read", "next", "done"}

TypeOK ==
    /\ dirty \in [Queues -> BOOLEAN]
    /\ marks \in [Queues -> BOOLEAN]
    /\ onRing \in [Queues -> BOOLEAN]
    /\ counted \in [Queues -> BOOLEAN]
    /\ inTable \in [Queues -> BOOLEAN]
    /\ asleep \in [Queues -> BOOLEAN]
    /\ pending \in [Queues -> BOOLEAN]
    /\ wpc \in [Queues -> WaiterSteps]
    /\ reports \in [Queues -> 0..Reports]
    /\ eager \subseteq Queues
    /\ cpc \in ChangerSteps
    /\ idx \in 1..(Len(Order) + 1)
    /\ made \in 0..Changes
    /\ stop \in 0..Len(Order)
    /\ delivered \in [Queues -> BOOLEAN]

\* Each queue's thread waits or does not, the model's choice.
Init ==
    /\ dirty = [q \in Queues |-> FALSE]
    /\ marks = [q \in Queues |-> FALSE]
    /\ onRing = [q \in Queues |-> FALSE]
    /\ counted = [q \in Queues |-> FALSE]
    /\ inTable = [q \in Queues |-> FALSE]
    /\ asleep = [q \in Queues |-> FALSE]
    /\ pending = [q \in Queues |-> FALSE]
    /\ eager \in SUBSET Queues
    /\ wpc = [q \in Queues |-> IF q \in eager THEN "peek" ELSE "none"]
    /\ reports = [q \in Queues |-> 0]
    /\ cpc = "change"
    /\ idx = 1
    /\ made = 0
    /\ stop = 0
    /\ delivered = [q \in Queues |-> FALSE]

\* `wake_all` on one queue's table: its thread, if queued, runs; one not
\* asleep yet keeps the wake for its block.
WakeAll(q) ==
    /\ asleep' = [asleep EXCEPT ![q] = IF inTable[q] THEN FALSE ELSE asleep[q]]
    /\ pending' = [pending EXCEPT ![q] = IF inTable[q] /\ ~asleep[q] THEN TRUE ELSE pending[q]]
    /\ inTable' = [inTable EXCEPT ![q] = FALSE]

----------------------------------------------------------------------------
(* A queue's thread: `wait`'s look, then `wait_ready`'s wait.              *)

\* The ring without its lock: empty means nothing to report, and a wait.
Peek(q) ==
    /\ wpc[q] = "peek"
    /\ wpc' = [wpc EXCEPT ![q] = IF onRing[q] THEN "pop" ELSE "count"]
    /\ UNCHANGED <<dirty, marks, onRing, counted, inTable, asleep, pending, reports, eager, cpc,
                   idx, made, stop, delivered>>

Pop(q) ==
    /\ wpc[q] = "pop"
    /\ onRing' = [onRing EXCEPT ![q] = FALSE]
    /\ wpc' = [wpc EXCEPT ![q] = "clear"]
    /\ UNCHANGED <<dirty, marks, counted, inTable, asleep, pending, reports, eager, cpc, idx,
                   made, stop, delivered>>

\* The queued mark is cleared before the description is looked at.
Clear(q) ==
    /\ wpc[q] = "clear"
    /\ marks' = [marks EXCEPT ![q] = FALSE]
    /\ wpc' = [wpc EXCEPT ![q] = "look"]
    /\ UNCHANGED <<dirty, onRing, counted, inTable, asleep, pending, reports, eager, cpc, idx,
                   made, stop, delivered>>

\* The look: a change it finds is reported; the call ends once it has
\* reported `Reports` times.
Look(q) ==
    /\ wpc[q] = "look"
    /\ IF dirty[q]
       THEN /\ dirty' = [dirty EXCEPT ![q] = FALSE]
            /\ reports' = [reports EXCEPT ![q] = @ + 1]
            /\ wpc' = [wpc EXCEPT ![q] = IF reports[q] + 1 = Reports THEN "done" ELSE "peek"]
       ELSE /\ wpc' = [wpc EXCEPT ![q] = "peek"]
            /\ UNCHANGED <<dirty, reports>>
    /\ UNCHANGED <<marks, onRing, counted, inTable, asleep, pending, eager, cpc, idx, made, stop,
                   delivered>>

\* `WaitCount::wait_until`: counted, with a full barrier after.
Count(q) ==
    /\ wpc[q] = "count"
    /\ counted' = [counted EXCEPT ![q] = TRUE]
    /\ wpc' = [wpc EXCEPT ![q] = "wait"]
    /\ UNCHANGED <<dirty, marks, onRing, inTable, asleep, pending, reports, eager, cpc, idx,
                   made, stop, delivered>>

\* `wait_until`'s critical section: the ring, then the queue.
Wait(q) ==
    /\ wpc[q] = "wait"
    /\ IF onRing[q]
       THEN /\ wpc' = [wpc EXCEPT ![q] = "uncount"]
            /\ UNCHANGED inTable
       ELSE /\ inTable' = [inTable EXCEPT ![q] = TRUE]
            /\ wpc' = [wpc EXCEPT ![q] = "block"]
    /\ UNCHANGED <<dirty, marks, onRing, counted, asleep, pending, reports, eager, cpc, idx, made,
                   stop, delivered>>

Block(q) ==
    /\ wpc[q] = "block"
    /\ IF pending[q]
       THEN /\ pending' = [pending EXCEPT ![q] = FALSE]
            /\ wpc' = [wpc EXCEPT ![q] = "woken"]
            /\ UNCHANGED asleep
       ELSE /\ asleep' = [asleep EXCEPT ![q] = TRUE]
            /\ wpc' = [wpc EXCEPT ![q] = "asleep"]
            /\ UNCHANGED pending
    /\ UNCHANGED <<dirty, marks, onRing, counted, inTable, reports, eager, cpc, idx, made, stop,
                   delivered>>

Wake(q) ==
    /\ wpc[q] = "asleep" /\ ~asleep[q]
    /\ wpc' = [wpc EXCEPT ![q] = "woken"]
    /\ UNCHANGED <<dirty, marks, onRing, counted, inTable, asleep, pending, reports, eager, cpc,
                   idx, made, stop, delivered>>

\* Off the table, if `wake_all` has not taken it off, then the condition.
Woken(q) ==
    /\ wpc[q] = "woken"
    /\ inTable' = [inTable EXCEPT ![q] = FALSE]
    /\ wpc' = [wpc EXCEPT ![q] = "wait"]
    /\ UNCHANGED <<dirty, marks, onRing, counted, asleep, pending, reports, eager, cpc, idx, made,
                   stop, delivered>>

\* `wait_until` has returned: no longer counted, and `wait` looks again.
Uncount(q) ==
    /\ wpc[q] = "uncount"
    /\ counted' = [counted EXCEPT ![q] = FALSE]
    /\ wpc' = [wpc EXCEPT ![q] = "peek"]
    /\ UNCHANGED <<dirty, marks, onRing, inTable, asleep, pending, reports, eager, cpc, idx, made,
                   stop, delivered>>

Waiter(q) ==
    Peek(q) \/ Pop(q) \/ Clear(q) \/ Look(q) \/ Count(q) \/ Wait(q) \/ Block(q) \/ Wake(q)
    \/ Woken(q) \/ Uncount(q)

----------------------------------------------------------------------------
(* The changer: the change, then `Polled::changed_by_rmw`'s walk.          *)

\* The description changes: every watch of it has something to look at.
Change ==
    /\ cpc = "change"
    /\ dirty' = [q \in Queues |-> TRUE]
    /\ made' = made + 1
    /\ idx' = 1
    /\ stop' = 0
    /\ delivered' = [q \in Queues |-> FALSE]
    /\ cpc' = "visit"
    /\ UNCHANGED <<marks, onRing, counted, inTable, asleep, pending, wpc, reports, eager>>

\* The next watch: an exclusive one after the stop is left alone.
Visit ==
    /\ cpc = "visit"
    /\ IF idx > Len(Order)
       THEN /\ cpc' = "next"
            /\ UNCHANGED idx
       ELSE IF Order[idx] \in Exclusive /\ stop # 0
            THEN /\ idx' = idx + 1
                 /\ UNCHANGED cpc
            ELSE /\ cpc' = "mark"
                 /\ UNCHANGED idx
    /\ UNCHANGED <<dirty, marks, onRing, counted, inTable, asleep, pending, wpc, reports, eager,
                   made, stop, delivered>>

\* `changed`: the queued mark, set only on a watch with none; a watch
\* that has it is on the ring or being taken off it, and its look comes
\* after the change.
Mark ==
    /\ cpc = "mark"
    /\ LET q == Order[idx] IN
       IF marks[q]
       THEN /\ delivered' = [delivered EXCEPT ![q] = TRUE]
            /\ cpc' = "read"
            /\ UNCHANGED marks
       ELSE /\ marks' = [marks EXCEPT ![q] = TRUE]
            /\ cpc' = "push"
            /\ UNCHANGED delivered
    /\ UNCHANGED <<dirty, onRing, counted, inTable, asleep, pending, wpc, reports, eager, idx,
                   made, stop>>

\* Onto the ring, under its lock.
Push ==
    /\ cpc = "push"
    /\ onRing' = [onRing EXCEPT ![Order[idx]] = TRUE]
    /\ delivered' = [delivered EXCEPT ![Order[idx]] = TRUE]
    /\ cpc' = "check"
    /\ UNCHANGED <<dirty, marks, counted, inTable, asleep, pending, wpc, reports, eager, idx,
                   made, stop>>

\* `ready.wake()`'s read of the count, after its barrier.
Check ==
    /\ cpc = "check"
    /\ cpc' = IF counted[Order[idx]] THEN "wake" ELSE "read"
    /\ UNCHANGED <<dirty, marks, onRing, counted, inTable, asleep, pending, wpc, reports, eager,
                   idx, made, stop, delivered>>

Waking ==
    /\ cpc = "wake"
    /\ WakeAll(Order[idx])
    /\ cpc' = "read"
    /\ UNCHANGED <<dirty, marks, onRing, counted, wpc, reports, eager, idx, made, stop, delivered>>

\* What `changed` returns: whether a thread is counted. An exclusive watch
\* that returns true stops the walk's exclusive watches.
Read ==
    /\ cpc = "read"
    /\ stop' = IF Order[idx] \in Exclusive /\ stop = 0 /\ counted[Order[idx]] THEN idx ELSE stop
    /\ idx' = idx + 1
    /\ cpc' = "visit"
    /\ UNCHANGED <<dirty, marks, onRing, counted, inTable, asleep, pending, wpc, reports, eager,
                   made, delivered>>

\* The walk is over: the next change, if any.
Next_ ==
    /\ cpc = "next"
    /\ cpc' = IF made < Changes THEN "change" ELSE "done"
    /\ UNCHANGED <<dirty, marks, onRing, counted, inTable, asleep, pending, wpc, reports, eager,
                   idx, made, stop, delivered>>

Changer == Change \/ Visit \/ Mark \/ Push \/ Check \/ Waking \/ Read \/ Next_

----------------------------------------------------------------------------

Done ==
    /\ cpc = "done"
    /\ \A q \in Queues : wpc[q] \in {"none", "done"} \/ (wpc[q] = "asleep" /\ asleep[q])

Next ==
    \/ \E q \in Queues : Waiter(q)
    \/ Changer
    \/ Done /\ UNCHANGED vars

Spec ==
    /\ Init /\ [][Next]_vars
    /\ \A q \in Queues : WF_vars(Waiter(q))
    /\ WF_vars(Changer)

\* A thread asleep is queued and counted, and its queue's ring holds the
\* watch only if the walk is on its way to wake it.
NoLostWakeup ==
    \A q \in Queues :
        asleep[q] =>
            /\ inTable[q] /\ counted[q]
            /\ onRing[q] => (cpc \in {"check", "wake"} /\ Order[idx] = q)

\* No exclusive watch after the one that stopped the walk is queued by it.
NoQueueAfterStop ==
    stop > 0 =>
        \A i \in (stop + 1)..Len(Order) : Order[i] \in Exclusive => ~delivered[Order[i]]

\* What a finished walk did.
WalkQueues ==
    cpc = "next" =>
        /\ \A q \in Queues \ Exclusive : delivered[q]
        /\ stop > 0 => \A i \in 1..stop : Order[i] \in Exclusive => delivered[Order[i]]
        /\ stop = 0 => \A q \in Queues : delivered[q]

\* With a thread waiting on any queue, some thread reports.
SomeThreadReports == eager # {} ~> (\E q \in Queues : reports[q] > 0)

=============================================================================
