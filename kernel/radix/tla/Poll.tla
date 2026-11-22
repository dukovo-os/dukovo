\* SPDX-FileCopyrightText: 2026 The Dukovo contributors
\* SPDX-License-Identifier: Apache-2.0 OR MIT

-------------------------------- MODULE Poll --------------------------------
(***************************************************************************)
(* A model of how `poll` waits for the objects it looks at                 *)
(* (kernel/radix/src/user/posix/poll.rs: `wait_ready`, `Polling`,          *)
(* `sleep_until_changed`, `Polled::changed` and `wake_poller`, ADR 0558),  *)
(* with the scheduler's `block` and `wake` beneath them.                   *)
(*                                                                         *)
(* Each object starts not ready and is made ready once, by its own         *)
(* changer, which then calls `changed`: if the object's count of pollers   *)
(* is not zero, it takes the object's lock and, for each poller on the     *)
(* object's list, sets the poller's flag and wakes it unless the flag was  *)
(* set already. A poller first looks at its objects without adding itself  *)
(* anywhere, and returns if one is ready. Otherwise it adds itself to the  *)
(* list of each, one object at a time, each under that object's lock;      *)
(* then, until it finds one ready, it clears its flag, looks, and sleeps   *)
(* until its flag is set: it checks the flag, and if it is clear, blocks.  *)
(* A wake before the block makes the block return at once. Once it has     *)
(* found an object ready it takes itself off each list. Which objects each *)
(* poller looks at is chosen at the start, so one run checks every choice. *)
(*                                                                         *)
(* Each critical section of an object's lock that does nothing but add or  *)
(* remove, and each atomic access, is one step; a change's critical        *)
(* section is a step for each flag it sets and each wake, so the pollers'  *)
(* steps that take no lock come between them. A list's count is stored     *)
(* with the list under the lock, so the model reads the list for it. The   *)
(* accesses the argument relies on are ordered as sequential consistency   *)
(* orders them: the poller's flag is cleared by a sequentially consistent  *)
(* store followed by a full barrier, and `changed` has a full barrier      *)
(* between the change and its read of the count (or the change is itself   *)
(* such a read-modify-write, a notification's), so the model's             *)
(* interleaving of single accesses covers every execution. Readiness only  *)
(* grows, so a look at the objects one after another finds what one look   *)
(* at them all, at its start or at its end, would: a look is one step.     *)
(* Signals and deadlines end a sleep sooner, by the same check, and are    *)
(* left out.                                                               *)
(*                                                                         *)
(* NoLostWakeup says a poller never sleeps while an object it looks at is  *)
(* ready, unless a changer is still on its way to wake it; EveryPollEnds   *)
(* says every poll ends, as every object is made ready in the end.         *)
(* NoLostWakeup fails if a poller clears its flag after it looks, or if    *)
(* `changed` reads the count before the change.                            *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    Pollers,    \* threads that poll
    Objects     \* what they look at, each made ready once by its changer

VARIABLES
    watch,      \* the objects each poller looks at, chosen at the start
    ready,      \* each object is ready
    list,       \* the pollers each object's `Polled` holds
    lock,       \* who holds each object's lock: "free", "changer" or a poller
    flag,       \* each poller's `poll_changed`
    asleep,     \* the scheduler has the poller blocked
    pending,    \* the scheduler holds a wake for its next block
    todo,       \* the objects each poller has still to add itself to or leave
    left,       \* the pollers each changer has still to tell
    cur,        \* the poller each changer is waking
    ppc,        \* where each poller is
    cpc         \* where each object's changer is

vars == <<watch, ready, list, lock, flag, asleep, pending, todo, left, cur, ppc, cpc>>

PollerSteps == {"first", "add", "clear", "look", "check", "block", "asleep",
                "remove", "done"}
ChangerSteps == {"change", "read", "lock", "next", "wake", "done"}

TypeOK ==
    /\ watch \in [Pollers -> SUBSET Objects]
    /\ ready \in [Objects -> BOOLEAN]
    /\ list \in [Objects -> SUBSET Pollers]
    /\ lock \in [Objects -> {"free", "changer"} \cup Pollers]
    /\ flag \in [Pollers -> BOOLEAN]
    /\ asleep \in [Pollers -> BOOLEAN]
    /\ pending \in [Pollers -> BOOLEAN]
    /\ todo \in [Pollers -> SUBSET Objects]
    /\ left \in [Objects -> SUBSET Pollers]
    /\ cur \in [Objects -> Pollers \cup {"none"}]
    /\ ppc \in [Pollers -> PollerSteps]
    /\ cpc \in [Objects -> ChangerSteps]

Init ==
    /\ watch \in [Pollers -> SUBSET Objects \ {{}}]
    /\ ready = [o \in Objects |-> FALSE]
    /\ list = [o \in Objects |-> {}]
    /\ lock = [o \in Objects |-> "free"]
    /\ flag = [p \in Pollers |-> FALSE]
    /\ asleep = [p \in Pollers |-> FALSE]
    /\ pending = [p \in Pollers |-> FALSE]
    /\ todo = [p \in Pollers |-> {}]
    /\ left = [o \in Objects |-> {}]
    /\ cur = [o \in Objects |-> "none"]
    /\ ppc = [p \in Pollers |-> "first"]
    /\ cpc = [o \in Objects |-> "change"]

AnyReady(p) == \E o \in watch[p] : ready[o]

----------------------------------------------------------------------------
(* A poller (`poll`, `wait_ready` and `sleep_until_changed`).              *)

\* The first look, added nowhere.
First(p) ==
    /\ ppc[p] = "first"
    /\ IF AnyReady(p)
       THEN /\ ppc' = [ppc EXCEPT ![p] = "done"]
            /\ UNCHANGED todo
       ELSE /\ ppc' = [ppc EXCEPT ![p] = "add"]
            /\ todo' = [todo EXCEPT ![p] = watch[p]]
    /\ UNCHANGED <<watch, ready, list, lock, flag, asleep, pending, left, cur, cpc>>

\* `Polling::new`: onto one object's list at a time, under its lock, in
\* the order `polled_by` names them (any one order will do).
Add(p) ==
    /\ ppc[p] = "add"
    /\ LET o == CHOOSE o \in todo[p] : TRUE IN
          /\ lock[o] = "free"
          /\ list' = [list EXCEPT ![o] = list[o] \cup {p}]
          /\ todo' = [todo EXCEPT ![p] = todo[p] \ {o}]
          /\ ppc' = [ppc EXCEPT ![p] = IF todo[p] = {o} THEN "clear" ELSE "add"]
    /\ UNCHANGED <<watch, ready, lock, flag, asleep, pending, left, cur, cpc>>

\* The flag cleared, and the barrier taken, before each look.
Clear(p) ==
    /\ ppc[p] = "clear"
    /\ flag' = [flag EXCEPT ![p] = FALSE]
    /\ ppc' = [ppc EXCEPT ![p] = "look"]
    /\ UNCHANGED <<watch, ready, list, lock, asleep, pending, todo, left, cur, cpc>>

Look(p) ==
    /\ ppc[p] = "look"
    /\ IF AnyReady(p)
       THEN /\ ppc' = [ppc EXCEPT ![p] = "remove"]
            /\ todo' = [todo EXCEPT ![p] = watch[p]]
       ELSE /\ ppc' = [ppc EXCEPT ![p] = "check"]
            /\ UNCHANGED todo
    /\ UNCHANGED <<watch, ready, list, lock, flag, asleep, pending, left, cur, cpc>>

\* `sleep_until_changed`'s check: told to look again, or block.
Check(p) ==
    /\ ppc[p] = "check"
    /\ ppc' = [ppc EXCEPT ![p] = IF flag[p] THEN "clear" ELSE "block"]
    /\ UNCHANGED <<watch, ready, list, lock, flag, asleep, pending, todo, left, cur, cpc>>

Block(p) ==
    /\ ppc[p] = "block"
    /\ IF pending[p]
       THEN /\ pending' = [pending EXCEPT ![p] = FALSE]
            /\ ppc' = [ppc EXCEPT ![p] = "check"]
            /\ UNCHANGED asleep
       ELSE /\ asleep' = [asleep EXCEPT ![p] = TRUE]
            /\ ppc' = [ppc EXCEPT ![p] = "asleep"]
            /\ UNCHANGED pending
    /\ UNCHANGED <<watch, ready, list, lock, flag, todo, left, cur, cpc>>

\* Woken: the check again.
Awake(p) ==
    /\ ppc[p] = "asleep" /\ ~asleep[p]
    /\ ppc' = [ppc EXCEPT ![p] = "check"]
    /\ UNCHANGED <<watch, ready, list, lock, flag, asleep, pending, todo, left, cur, cpc>>

\* `Polling`'s drop: off one object's list at a time, under its lock.
Remove(p) ==
    /\ ppc[p] = "remove"
    /\ LET o == CHOOSE o \in todo[p] : TRUE IN
          /\ lock[o] = "free"
          /\ list' = [list EXCEPT ![o] = list[o] \ {p}]
          /\ todo' = [todo EXCEPT ![p] = todo[p] \ {o}]
          /\ ppc' = [ppc EXCEPT ![p] = IF todo[p] = {o} THEN "done" ELSE "remove"]
    /\ UNCHANGED <<watch, ready, lock, flag, asleep, pending, left, cur, cpc>>

Poller(p) ==
    First(p) \/ Add(p) \/ Clear(p) \/ Look(p) \/ Check(p) \/ Block(p) \/ Awake(p)
    \/ Remove(p)

----------------------------------------------------------------------------
(* An object's changer: the change, then `Polled::changed`.                *)

Change(o) ==
    /\ cpc[o] = "change"
    /\ ready' = [ready EXCEPT ![o] = TRUE]
    /\ cpc' = [cpc EXCEPT ![o] = "read"]
    /\ UNCHANGED <<watch, list, lock, flag, asleep, pending, todo, left, cur, ppc>>

\* The count of pollers, read without the lock.
Read(o) ==
    /\ cpc[o] = "read"
    /\ cpc' = [cpc EXCEPT ![o] = IF list[o] # {} THEN "lock" ELSE "done"]
    /\ UNCHANGED <<watch, ready, list, lock, flag, asleep, pending, todo, left, cur, ppc>>

Lock(o) ==
    /\ cpc[o] = "lock"
    /\ lock[o] = "free"
    /\ lock' = [lock EXCEPT ![o] = "changer"]
    /\ left' = [left EXCEPT ![o] = list[o]]
    /\ cpc' = [cpc EXCEPT ![o] = "next"]
    /\ UNCHANGED <<watch, ready, list, flag, asleep, pending, todo, cur, ppc>>

\* `wake_poller`'s swap of the next poller's flag, in the list's order (any
\* one order will do); the lock goes once every poller on the list has been
\* told.
Next(o) ==
    /\ cpc[o] = "next"
    /\ IF left[o] = {}
       THEN /\ lock' = [lock EXCEPT ![o] = "free"]
            /\ cpc' = [cpc EXCEPT ![o] = "done"]
            /\ UNCHANGED <<flag, left, cur>>
       ELSE LET q == CHOOSE q \in left[o] : TRUE IN
               IF flag[q]
               THEN /\ left' = [left EXCEPT ![o] = left[o] \ {q}]
                    /\ UNCHANGED <<flag, cur, cpc, lock>>
               ELSE /\ flag' = [flag EXCEPT ![q] = TRUE]
                    /\ cur' = [cur EXCEPT ![o] = q]
                    /\ cpc' = [cpc EXCEPT ![o] = "wake"]
                    /\ UNCHANGED <<left, lock>>
    /\ UNCHANGED <<watch, ready, list, asleep, pending, todo, ppc>>

\* `sched::wake`: a poller not asleep yet keeps the wake for its block.
Wake(o) ==
    /\ cpc[o] = "wake"
    /\ LET q == cur[o] IN
          /\ asleep' = [asleep EXCEPT ![q] = FALSE]
          /\ pending' = [pending EXCEPT ![q] = ~asleep[q]]
          /\ left' = [left EXCEPT ![o] = left[o] \ {q}]
    /\ cur' = [cur EXCEPT ![o] = "none"]
    /\ cpc' = [cpc EXCEPT ![o] = "next"]
    /\ UNCHANGED <<watch, ready, list, lock, flag, todo, ppc>>

Changer(o) == Change(o) \/ Read(o) \/ Lock(o) \/ Next(o) \/ Wake(o)

----------------------------------------------------------------------------

Done == (\A p \in Pollers : ppc[p] = "done") /\ (\A o \in Objects : cpc[o] = "done")

NextStep ==
    \/ \E p \in Pollers : Poller(p)
    \/ \E o \in Objects : Changer(o)
    \/ Done /\ UNCHANGED vars

Spec ==
    /\ Init /\ [][NextStep]_vars
    /\ \A p \in Pollers : WF_vars(Poller(p))
    /\ \A o \in Objects : WF_vars(Changer(o))

\* A changer of `o` will still wake `p`: it has the count to read or the
\* lock to take with `p` on the list, `p` is among those it has still to
\* tell and its flag is clear, or it is waking `p` now.
OnItsWay(o, p) ==
    \/ cpc[o] \in {"read", "lock"} /\ p \in list[o]
    \/ cpc[o] \in {"next", "wake"} /\ p \in left[o] /\ (~flag[p] \/ cur[o] = p)

\* Asleep, a poller is on every list it looks at, and if one of its objects
\* is ready, some changer is still on its way to wake it.
NoLostWakeup ==
    \A p \in Pollers :
        asleep[p] =>
            /\ \A o \in watch[p] : p \in list[o]
            /\ AnyReady(p) => \E o \in Objects : OnItsWay(o, p)

\* A poller past adding itself, and not yet leaving, is on every list it
\* looks at.
Added ==
    \A p \in Pollers :
        ppc[p] \in {"clear", "look", "check", "block", "asleep"} =>
            \A o \in watch[p] : p \in list[o]

\* A change tells only the pollers on its object's list.
OnlyListed ==
    \A o \in Objects : left[o] \subseteq list[o] /\ (cur[o] \in Pollers => cur[o] \in list[o])

\* Every poll ends.
EveryPollEnds == \A p \in Pollers : <>(ppc[p] = "done")

=============================================================================
