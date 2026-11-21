\* SPDX-FileCopyrightText: 2026 The Dukovo contributors
\* SPDX-License-Identifier: Apache-2.0 OR MIT

----------------------------- MODULE EventQueue -----------------------------
(***************************************************************************)
(* A model of an event queue's waits (kernel/radix/src/user/queue.rs:      *)
(* `changed`, `EventQueue::look`, `wait_ready` and `Registry::delete`),    *)
(* with the wait table's `wait_until` and `wake_all` (sched/wait.rs) and   *)
(* the scheduler's `block` and `wake` beneath them, as Poll.tla has them.  *)
(*                                                                         *)
(* Each watch watches its own object, which its changer changes a few      *)
(* times: each change makes the object ready and leaves something for a    *)
(* look to see (`dirty`). After a change, `Polled::changed` calls          *)
(* `changed`: if the watch has neither mark, it is marked queued, pushed   *)
(* on the ring, and, if a waiter is counted (`WaitCount`), every waiter is *)
(* woken. A waiter's call finds the ring empty without the lock, or takes  *)
(* the queue's lock, notes how many watches are on the ring, and takes     *)
(* that many off, one at a time, until it has reported Capacity of them:   *)
(* it clears a watch's queued mark, and drops a deleted one, giving back   *)
(* its room; or else it looks at the object, and reports it if it is       *)
(* ready, as the program then reads all it holds. A level-triggered watch  *)
(* it reported goes back on the ring, and counted waiters are woken. A     *)
(* call that reported nothing counts itself, then waits until the ring     *)
(* holds a watch, no longer counted once the wait ends, and looks again. A *)
(* deleter deletes one watch, with the queue's lock held: it marks it      *)
(* deleted, and gives back its room unless it is queued. Every watch is    *)
(* added before the model starts, and queued for a first look, as `add`    *)
(* does.                                                                   *)
(*                                                                         *)
(* Each critical section of a lock and each atomic access is one step, and *)
(* the accesses are ordered as sequential consistency orders them: the     *)
(* marks and the count change by sequentially consistent                   *)
(* read-modify-writes, with full barriers after a change, after a look's   *)
(* clear, after a waiter counts itself and after a push, so the model's    *)
(* interleaving covers every execution. Signals and deadlines end a wait   *)
(* sooner, by the same condition, and are left out.                        *)
(*                                                                         *)
(* NoLostWakeup says a waiter never sleeps while a watch is on the ring,   *)
(* unless whoever put it there is still on its way to wake it, nor while   *)
(* a change no look has seen is left with nothing to bring it to one.     *)
(* NoReportAfterDelete says a watch is never reported once its delete has  *)
(* returned, AtMostOnce that a watch is on the ring at most once, and      *)
(* RoomExact that the room the queue keeps on its ring counts exactly the  *)
(* watches that may be on it, so that `changed` never finds the ring full. *)
(* EveryChangeLooked says every change to a watch that is not deleted is   *)
(* looked at in the end. NoLostWakeup fails if a look reads the object     *)
(* before it clears the queued mark, or a waiter counts itself after its   *)
(* condition looks at the ring, and NoReportAfterDelete if a call lets the *)
(* lock go between taking a watch off and reporting it.                    *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    Watches,    \* the queue's watches, each of its own object
    Level,      \* those that are level-triggered; the others have `clear`
    Waiters,    \* threads that call `wait`
    Changes,    \* how many times each object changes
    Capacity    \* how many events one call reports at most

ASSUME Level \subseteq Watches /\ Changes \in Nat /\ Capacity \in Nat \ {0}

VARIABLES
    ready,      \* each object holds something to read
    dirty,      \* each object changed since a look last saw it
    marks,      \* each watch's marks: "queued", "deleted"
    ring,       \* the ready ring
    room,       \* how many watches may be on the ring: `on_ring_max`
    owner,      \* who holds the queue's lock: a waiter, "deleter" or "none"
    queuedW,    \* the waiters queued in the wait table on the ring's key
    counted,    \* the waiters counted in the queue's `WaitCount`
    asleep,     \* the scheduler has the waiter blocked
    pending,    \* the scheduler holds a wake for its next block
    hand,       \* the watch a waiter has taken off the ring, or "none"
    left,       \* how many more the waiter's call takes off the ring
    found,      \* how many the waiter's call has reported
    wpc,        \* where each waiter is
    made,       \* how many changes each changer has made
    cpc,        \* where each changer is
    victim,     \* the watch the deleter deletes
    deleted,    \* the watches whose delete has returned
    badReport,  \* a watch was reported after its delete returned
    dpc         \* where the deleter is

vars == <<ready, dirty, marks, ring, room, owner, queuedW, counted, asleep, pending, hand, left,
          found, wpc, made, cpc, victim, deleted, badReport, dpc>>

None == "none"

WaiterSteps == {"peek", "lock", "pop", "clear", "look", "rcheck", "rwake", "unlock", "count",
                "wait", "uncount", "block", "asleep", "woken"}
ChangerSteps == {"change", "mark", "push", "check", "wake", "next", "done"}
DeleterSteps == {"lock", "delete", "done"}

InRing(w) == \E i \in 1..Len(ring) : ring[i] = w

TypeOK ==
    /\ ready \in [Watches -> BOOLEAN]
    /\ dirty \in [Watches -> BOOLEAN]
    /\ marks \in [Watches -> SUBSET {"queued", "deleted"}]
    /\ ring \in Seq(Watches)
    /\ room \in 0..Cardinality(Watches)
    /\ owner \in Waiters \cup {"deleter", None}
    /\ queuedW \subseteq Waiters
    /\ counted \subseteq Waiters
    /\ asleep \in [Waiters -> BOOLEAN]
    /\ pending \in [Waiters -> BOOLEAN]
    /\ hand \in [Waiters -> Watches \cup {None}]
    /\ left \in [Waiters -> 0..Cardinality(Watches)]
    /\ found \in [Waiters -> 0..Capacity]
    /\ wpc \in [Waiters -> WaiterSteps]
    /\ made \in [Watches -> 0..Changes]
    /\ cpc \in [Watches -> ChangerSteps]
    /\ victim \in Watches
    /\ deleted \subseteq Watches
    /\ badReport \in BOOLEAN
    /\ dpc \in DeleterSteps

\* Every watch is added, and queued for a first look, before the start.
Init ==
    /\ ready = [w \in Watches |-> FALSE]
    /\ dirty = [w \in Watches |-> FALSE]
    /\ marks = [w \in Watches |-> {"queued"}]
    /\ ring \in {s \in [1..Cardinality(Watches) -> Watches] :
                    \A w \in Watches : \E i \in DOMAIN s : s[i] = w}
    /\ room = Cardinality(Watches)
    /\ owner = None
    /\ queuedW = {}
    /\ counted = {}
    /\ asleep = [p \in Waiters |-> FALSE]
    /\ pending = [p \in Waiters |-> FALSE]
    /\ hand = [p \in Waiters |-> None]
    /\ left = [p \in Waiters |-> 0]
    /\ found = [p \in Waiters |-> 0]
    /\ wpc = [p \in Waiters |-> "peek"]
    /\ made = [w \in Watches |-> 0]
    /\ cpc = [w \in Watches |-> IF Changes > 0 THEN "change" ELSE "done"]
    /\ victim \in Watches
    /\ deleted = {}
    /\ badReport = FALSE
    /\ dpc = "lock"

\* `wake_all`'s critical section: every queued waiter off the table and
\* woken; one not asleep yet keeps the wake for its block.
WakeAll ==
    /\ asleep' = [p \in Waiters |-> IF p \in queuedW THEN FALSE ELSE asleep[p]]
    /\ pending' = [p \in Waiters |-> IF p \in queuedW /\ ~asleep[p] THEN TRUE ELSE pending[p]]
    /\ queuedW' = {}

----------------------------------------------------------------------------
(* A waiter: `wait`'s look (`EventQueue::look`), then `wait_ready`'s wait.  *)

\* `has_queued`, without the lock: a call that finds the ring empty
\* reports nothing, and waits.
Peek(p) ==
    /\ wpc[p] = "peek"
    /\ found' = [found EXCEPT ![p] = 0]
    /\ wpc' = [wpc EXCEPT ![p] = IF ring = <<>> THEN "count" ELSE "lock"]
    /\ UNCHANGED <<counted, ready, dirty, marks, ring, room, owner, queuedW, asleep, pending,
                   hand, left, made, cpc, victim, deleted, badReport, dpc>>

Lock(p) ==
    /\ wpc[p] = "lock" /\ owner = None
    /\ owner' = p
    /\ left' = [left EXCEPT ![p] = Len(ring)]
    /\ found' = [found EXCEPT ![p] = 0]
    /\ wpc' = [wpc EXCEPT ![p] = "pop"]
    /\ UNCHANGED <<counted, ready, dirty, marks, ring, room, queuedW, asleep, pending, hand, made, cpc,
                   victim, deleted, badReport, dpc>>

\* `Ready::pop`, under the ring's lock.
Pop(p) ==
    /\ wpc[p] = "pop"
    /\ IF left[p] > 0 /\ found[p] < Capacity /\ ring # <<>>
       THEN /\ hand' = [hand EXCEPT ![p] = Head(ring)]
            /\ ring' = Tail(ring)
            /\ left' = [left EXCEPT ![p] = left[p] - 1]
            /\ wpc' = [wpc EXCEPT ![p] = "clear"]
       ELSE /\ wpc' = [wpc EXCEPT ![p] = "unlock"]
            /\ UNCHANGED <<counted, hand, ring, left>>
    /\ UNCHANGED <<counted, ready, dirty, marks, room, owner, queuedW, asleep, pending, found, made, cpc,
                   victim, deleted, badReport, dpc>>

\* The queued mark cleared; a deleted watch gives back its room.
Clear(p) ==
    /\ wpc[p] = "clear"
    /\ LET w == hand[p] IN
       /\ marks' = [marks EXCEPT ![w] = @ \ {"queued"}]
       /\ IF "deleted" \in marks[w]
          THEN /\ room' = room - 1
               /\ hand' = [hand EXCEPT ![p] = None]
               /\ wpc' = [wpc EXCEPT ![p] = "pop"]
          ELSE /\ wpc' = [wpc EXCEPT ![p] = "look"]
               /\ UNCHANGED <<counted, room, hand>>
    /\ UNCHANGED <<counted, ready, dirty, ring, owner, queuedW, asleep, pending, left, found, made, cpc,
                   victim, deleted, badReport, dpc>>

\* The look, and the report of a ready object, which the program then
\* reads; a level-triggered watch it reports goes back on the ring
\* (`changed`, whose mark and push are one step here: the lock held keeps
\* deletes out, and a changer that marks it first pushes it itself).
Look(p) ==
    /\ wpc[p] = "look"
    /\ LET w == hand[p]
           reported == ready[w]
           again == reported /\ w \in Level /\ marks[w] = {}
       IN
       /\ dirty' = [dirty EXCEPT ![w] = FALSE]
       /\ ready' = [ready EXCEPT ![w] = FALSE]
       /\ found' = [found EXCEPT ![p] = IF reported THEN found[p] + 1 ELSE found[p]]
       /\ badReport' = (badReport \/ (reported /\ w \in deleted))
       /\ marks' = IF again THEN [marks EXCEPT ![w] = {"queued"}] ELSE marks
       /\ ring' = IF again THEN Append(ring, w) ELSE ring
       /\ hand' = [hand EXCEPT ![p] = None]
       /\ wpc' = [wpc EXCEPT ![p] = IF again THEN "rcheck" ELSE "pop"]
    /\ UNCHANGED <<counted, room, owner, queuedW, asleep, pending, left, made, cpc, victim, deleted,
                   dpc>>

\* `WaitCount::wake_all`'s read of the count, after its barrier, for a
\* watch the look put back: a wake only if a waiter is counted.
RequeueCheck(p) ==
    /\ wpc[p] = "rcheck"
    /\ wpc' = [wpc EXCEPT ![p] = IF counted # {} THEN "rwake" ELSE "pop"]
    /\ UNCHANGED <<counted, ready, dirty, marks, ring, room, owner, queuedW, asleep, pending,
                   hand, left, found, made, cpc, victim, deleted, badReport, dpc>>

\* `changed`'s wake, for a watch the look put back.
RequeueWake(p) ==
    /\ wpc[p] = "rwake"
    /\ WakeAll
    /\ wpc' = [wpc EXCEPT ![p] = "pop"]
    /\ UNCHANGED <<counted, ready, dirty, marks, ring, room, owner, hand, left, found, made, cpc,
                   victim, deleted, badReport, dpc>>

\* The lock let go: a call that reported something returns, and the
\* program calls again; one that reported nothing waits.
Unlock(p) ==
    /\ wpc[p] = "unlock"
    /\ owner' = None
    /\ wpc' = [wpc EXCEPT ![p] = IF found[p] > 0 THEN "peek" ELSE "count"]
    /\ UNCHANGED <<counted, ready, dirty, marks, ring, room, queuedW, asleep, pending, hand, left, found,
                   made, cpc, victim, deleted, badReport, dpc>>

\* `WaitCount::wait_until`: counted, with a full barrier after.
Count(p) ==
    /\ wpc[p] = "count"
    /\ counted' = counted \cup {p}
    /\ wpc' = [wpc EXCEPT ![p] = "wait"]
    /\ UNCHANGED <<ready, dirty, marks, ring, room, owner, queuedW, asleep, pending, hand, left,
                   found, made, cpc, victim, deleted, badReport, dpc>>

\* `wait_until`'s critical section: the condition, `has_queued`, then the
\* queue.
Wait(p) ==
    /\ wpc[p] = "wait"
    /\ IF ring # <<>>
       THEN /\ wpc' = [wpc EXCEPT ![p] = "uncount"]
            /\ UNCHANGED queuedW
       ELSE /\ queuedW' = queuedW \cup {p}
            /\ wpc' = [wpc EXCEPT ![p] = "block"]
    /\ UNCHANGED <<counted, ready, dirty, marks, ring, room, owner, asleep, pending, hand, left, found,
                   made, cpc, victim, deleted, badReport, dpc>>

Block(p) ==
    /\ wpc[p] = "block"
    /\ IF pending[p]
       THEN /\ pending' = [pending EXCEPT ![p] = FALSE]
            /\ wpc' = [wpc EXCEPT ![p] = "woken"]
            /\ UNCHANGED asleep
       ELSE /\ asleep' = [asleep EXCEPT ![p] = TRUE]
            /\ wpc' = [wpc EXCEPT ![p] = "asleep"]
            /\ UNCHANGED pending
    /\ UNCHANGED <<counted, ready, dirty, marks, ring, room, owner, queuedW, hand, left, found, made, cpc,
                   victim, deleted, badReport, dpc>>

\* `wait_until` has returned: no longer counted, and `wait` looks again.
Uncount(p) ==
    /\ wpc[p] = "uncount"
    /\ counted' = counted \ {p}
    /\ wpc' = [wpc EXCEPT ![p] = "peek"]
    /\ UNCHANGED <<ready, dirty, marks, ring, room, owner, queuedW, asleep, pending, hand, left,
                   found, made, cpc, victim, deleted, badReport, dpc>>

Wake(p) ==
    /\ wpc[p] = "asleep" /\ ~asleep[p]
    /\ wpc' = [wpc EXCEPT ![p] = "woken"]
    /\ UNCHANGED <<counted, ready, dirty, marks, ring, room, owner, queuedW, asleep, pending, hand, left,
                   found, made, cpc, victim, deleted, badReport, dpc>>

\* Off the table (if `wake_all` has not taken it off), then the condition
\* again.
Woken(p) ==
    /\ wpc[p] = "woken"
    /\ queuedW' = queuedW \ {p}
    /\ wpc' = [wpc EXCEPT ![p] = "wait"]
    /\ UNCHANGED <<counted, ready, dirty, marks, ring, room, owner, asleep, pending, hand, left, found,
                   made, cpc, victim, deleted, badReport, dpc>>

Waiter(p) ==
    Peek(p) \/ Lock(p) \/ Pop(p) \/ Clear(p) \/ Look(p) \/ RequeueCheck(p) \/ RequeueWake(p)
    \/ Unlock(p) \/ Count(p) \/ Wait(p) \/ Uncount(p) \/ Block(p) \/ Wake(p) \/ Woken(p)

----------------------------------------------------------------------------
(* An object's changer: the change, then `Polled::changed` and `changed`.  *)

Change(w) ==
    /\ cpc[w] = "change"
    /\ ready' = [ready EXCEPT ![w] = TRUE]
    /\ dirty' = [dirty EXCEPT ![w] = TRUE]
    /\ made' = [made EXCEPT ![w] = made[w] + 1]
    /\ cpc' = [cpc EXCEPT ![w] = "mark"]
    /\ UNCHANGED <<counted, marks, ring, room, owner, queuedW, asleep, pending, hand, left, found, wpc,
                   victim, deleted, badReport, dpc>>

\* The mark, set only on a watch with none.
Mark(w) ==
    /\ cpc[w] = "mark"
    /\ IF marks[w] = {}
       THEN /\ marks' = [marks EXCEPT ![w] = {"queued"}]
            /\ cpc' = [cpc EXCEPT ![w] = "push"]
       ELSE /\ cpc' = [cpc EXCEPT ![w] = "next"]
            /\ UNCHANGED marks
    /\ UNCHANGED <<counted, ready, dirty, ring, room, owner, queuedW, asleep, pending, hand, left, found,
                   wpc, made, victim, deleted, badReport, dpc>>

\* Onto the ring, under its lock: the queue has made room.
Push(w) ==
    /\ cpc[w] = "push"
    /\ ring' = Append(ring, w)
    /\ cpc' = [cpc EXCEPT ![w] = "check"]
    /\ UNCHANGED <<counted, ready, dirty, marks, room, owner, queuedW, asleep, pending, hand, left, found,
                   wpc, made, victim, deleted, badReport, dpc>>

\* `WaitCount::wake_all`'s read of the count, after its barrier.
ChangerCheck(w) ==
    /\ cpc[w] = "check"
    /\ cpc' = [cpc EXCEPT ![w] = IF counted # {} THEN "wake" ELSE "next"]
    /\ UNCHANGED <<counted, ready, dirty, marks, ring, room, owner, queuedW, asleep, pending,
                   hand, left, found, wpc, made, victim, deleted, badReport, dpc>>

ChangerWake(w) ==
    /\ cpc[w] = "wake"
    /\ WakeAll
    /\ cpc' = [cpc EXCEPT ![w] = "next"]
    /\ UNCHANGED <<counted, ready, dirty, marks, ring, room, owner, hand, left, found, wpc, made, victim,
                   deleted, badReport, dpc>>

Next_(w) ==
    /\ cpc[w] = "next"
    /\ cpc' = [cpc EXCEPT ![w] = IF made[w] < Changes THEN "change" ELSE "done"]
    /\ UNCHANGED <<counted, ready, dirty, marks, ring, room, owner, queuedW, asleep, pending, hand, left,
                   found, wpc, made, victim, deleted, badReport, dpc>>

Changer(w) == Change(w) \/ Mark(w) \/ Push(w) \/ ChangerCheck(w) \/ ChangerWake(w) \/ Next_(w)

----------------------------------------------------------------------------
(* The deleter: a change with `delete`, under the queue's lock.            *)

DeleterLock ==
    /\ dpc = "lock" /\ owner = None
    /\ owner' = "deleter"
    /\ dpc' = "delete"
    /\ UNCHANGED <<counted, ready, dirty, marks, ring, room, queuedW, asleep, pending, hand, left, found,
                   wpc, made, cpc, victim, deleted, badReport>>

\* `Registry::delete`, and the lock let go: the delete has returned.
Delete ==
    /\ dpc = "delete"
    /\ marks' = [marks EXCEPT ![victim] = @ \cup {"deleted"}]
    /\ room' = IF "queued" \in marks[victim] THEN room ELSE room - 1
    /\ deleted' = deleted \cup {victim}
    /\ owner' = None
    /\ dpc' = "done"
    /\ UNCHANGED <<counted, ready, dirty, ring, queuedW, asleep, pending, hand, left, found, wpc, made,
                   cpc, victim, badReport>>

Deleter == DeleterLock \/ Delete

----------------------------------------------------------------------------

Done ==
    /\ \A w \in Watches : cpc[w] = "done"
    /\ dpc = "done"
    /\ \A p \in Waiters : wpc[p] = "asleep" /\ asleep[p]

Next ==
    \/ \E p \in Waiters : Waiter(p)
    \/ \E w \in Watches : Changer(w)
    \/ Deleter
    \/ Done /\ UNCHANGED vars

Spec ==
    /\ Init /\ [][Next]_vars
    /\ \A p \in Waiters : WF_vars(Waiter(p))
    /\ \A w \in Watches : WF_vars(Changer(w))
    /\ WF_vars(Deleter)

\* Whoever pushed a watch on the ring is still to read the count, or to
\* wake the waiters it found counted.
Waking ==
    \/ \E w \in Watches : cpc[w] \in {"check", "wake"}
    \/ \E p \in Waiters : wpc[p] \in {"rcheck", "rwake"}

\* Asleep, a waiter is queued; the ring is empty unless its pusher is
\* still to wake it; and each change no look has seen, to a watch not
\* deleted, is on the ring, in a call's hand, or with its changer on its
\* way to queue it.
NoLostWakeup ==
    \A p \in Waiters :
        asleep[p] =>
            /\ p \in queuedW
            /\ p \in counted
            /\ ring # <<>> => Waking
            /\ \A w \in Watches :
                  (dirty[w] /\ "deleted" \notin marks[w]) =>
                      \/ InRing(w)
                      \/ \E q \in Waiters : hand[q] = w
                      \/ cpc[w] \in {"mark", "push", "check", "wake"}

NoReportAfterDelete == ~badReport

AtMostOnce == \A i, j \in 1..Len(ring) : i # j => ring[i] # ring[j]

\* A watch on the ring is marked queued.
RingMarked == \A i \in 1..Len(ring) : "queued" \in marks[ring[i]]

\* The room counts the watches not deleted, and the deleted ones still
\* marked queued, so the ring always has room.
RoomExact ==
    /\ room = Cardinality({w \in Watches : "deleted" \notin marks[w] \/ "queued" \in marks[w]})
    /\ Len(ring) <= room

\* Every change to a watch not deleted is looked at.
EveryChangeLooked ==
    \A w \in Watches : [](dirty[w] => <>(~dirty[w] \/ "deleted" \in marks[w]))

=============================================================================
