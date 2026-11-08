\* SPDX-FileCopyrightText: 2026 The Dukovo contributors
\* SPDX-License-Identifier: Apache-2.0 OR MIT

----------------------------- MODULE LightBorrow -----------------------------
(***************************************************************************)
(* A model of the light borrow (ADR 0414): a call on a file or directory   *)
(* capability that uses the object its CPU's cache found without counting  *)
(* a reference to it (kernel/radix/src/user/objects.rs), and what keeps    *)
(* the object there until the call ends.                                   *)
(*                                                                         *)
(* One object, a directory, and two capabilities to it: `D0` in the space  *)
(* of a process `Q`, and `S`, derived from `D0`, in the space of `P`, as   *)
(* `fork` makes them. The store holds one reference to the object while    *)
(* any capability names it, and lets go of it when the last one goes.      *)
(*                                                                         *)
(* `T` is `P`'s thread. It calls a method on `S`: in one read section (no  *)
(* quiescent state inside) it finds the capability in `S` and, if `P` has  *)
(* no other thread and no revoke from a program is under way, borrows the  *)
(* object without counting; otherwise it counts a reference, as the store  *)
(* still holds one. It uses the object, then ends the call: a borrow that  *)
(* was counted meanwhile drops its reference. Before `T`'s CPU passes a    *)
(* quiescent state, which may happen at any point of the call outside the  *)
(* read section (a timer tick, a switch), the borrow is counted. `T` may   *)
(* also delete `S`, between calls, and start a second thread, `U`.         *)
(*                                                                         *)
(* `Q`'s threads may delete `D0`; revoke `D0`, which deletes `S`; and      *)
(* start `U` in `P`. A revoke says it is under way, waits for a grace      *)
(* period, deletes, and says it is done. A start counts `U` among `P`'s    *)
(* threads, waits for a grace period, and only then lets `U` run. Once     *)
(* running, `U` may delete `S` and exit. A grace period ends once `T`'s   *)
(* CPU has passed a quiescent state after it started.                      *)
(*                                                                         *)
(* NoUseAfterFree says nothing uses or holds the object once it is freed.  *)
(* LightBacked says a borrow not counted has its capability in `S`, so the *)
(* store's reference is there. ArcRight says the object's count is the     *)
(* references there are. Leaving out any of the three breaks the first two:*)
(* counting the borrow before a quiescent state, the revoke's grace period *)
(* (`Q` revokes, then deletes `D0`), or the start's (`U` deletes `S`).     *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    Qs          \* `Q`'s threads

VARIABLES
    caps,       \* the slots that hold a capability to the object
    arc,        \* the object's reference count
    freed,      \* whether the object is freed
    pcT,        \* where `T` is: "idle", or "use" in a call
    borrow,     \* `T`'s hold on the object: "none", "light" or "counted"
    threads,    \* `P`'s threads that have not exited
    u,          \* `U`: "none", "held" (counted, not running), "run", "gone"
    revokes,    \* revokes from a program under way
    pcQ,        \* where each of `Q`'s threads is
    waitFor     \* for each of `Q`'s threads, the CPUs its grace period waits for

vars == <<caps, arc, freed, pcT, borrow, threads, u, revokes, pcQ, waitFor>>

TypeOK ==
    /\ caps \subseteq {"S", "D0"}
    /\ arc \in 0..3
    /\ freed \in BOOLEAN
    /\ pcT \in {"idle", "use"}
    /\ borrow \in {"none", "light", "counted"}
    /\ threads \in 1..2
    /\ u \in {"none", "held", "run", "gone"}
    /\ revokes \in 0..2
    /\ pcQ \in [Qs -> {"idle", "revoke", "revoked", "start"}]
    /\ waitFor \in [Qs -> SUBSET {"T"}]

Init ==
    /\ caps = {"S", "D0"}
    /\ arc = 1
    /\ freed = FALSE
    /\ pcT = "idle"
    /\ borrow = "none"
    /\ threads = 1
    /\ u = "none"
    /\ revokes = 0
    /\ pcQ = [q \in Qs |-> "idle"]
    /\ waitFor = [q \in Qs |-> {}]

\* The capability in `slot` is deleted: the last one lets go of the
\* store's reference.
Delete(slot) ==
    /\ caps' = caps \ {slot}
    /\ IF caps' = {} /\ caps # {}
       THEN /\ arc' = arc - 1
            /\ freed' = (arc = 1)
       ELSE UNCHANGED <<arc, freed>>

----------------------------------------------------------------------------
(* `T`, `P`'s thread.                                                      *)

\* The read section: the capability found in `S`, and the object borrowed,
\* light if `P` has one thread and no revoke is under way.
Call ==
    /\ pcT = "idle" /\ "S" \in caps
    /\ IF threads = 1 /\ revokes = 0
       THEN /\ borrow' = "light"
            /\ UNCHANGED arc
       ELSE /\ borrow' = "counted"
            /\ arc' = arc + 1
    /\ pcT' = "use"
    /\ UNCHANGED <<caps, freed, threads, u, revokes, pcQ, waitFor>>

\* The call ends: a counted borrow drops its reference.
End ==
    /\ pcT = "use"
    /\ borrow' = "none"
    /\ pcT' = "idle"
    /\ IF borrow = "counted"
       THEN /\ arc' = arc - 1
            /\ freed' = (arc = 1)
       ELSE UNCHANGED <<arc, freed>>
    /\ UNCHANGED <<caps, threads, u, revokes, pcQ, waitFor>>

\* `T`'s CPU passes a quiescent state, anywhere but in a read section: the
\* borrow is counted first. Every grace period under way stops waiting
\* for it.
Quiescent ==
    /\ IF borrow = "light"
       THEN /\ borrow' = "counted"
            /\ arc' = arc + 1
       ELSE UNCHANGED <<borrow, arc>>
    /\ waitFor' = [q \in Qs |-> waitFor[q] \ {"T"}]
    /\ UNCHANGED <<caps, freed, pcT, threads, u, revokes, pcQ>>

\* `T` closes `S`, between calls.
CloseS ==
    /\ pcT = "idle" /\ "S" \in caps
    /\ Delete("S")
    /\ UNCHANGED <<pcT, borrow, threads, u, revokes, pcQ, waitFor>>

\* `T` starts `U`, between calls: it runs at once.
StartOwn ==
    /\ pcT = "idle" /\ u = "none"
    /\ threads' = 2
    /\ u' = "run"
    /\ UNCHANGED <<caps, arc, freed, pcT, borrow, revokes, pcQ, waitFor>>

----------------------------------------------------------------------------
(* `Q`'s threads.                                                          *)

\* A revoke of `D0` says it is under way, and its grace period starts.
RevokeStart(q) ==
    /\ pcQ[q] = "idle" /\ "D0" \in caps
    /\ revokes' = revokes + 1
    /\ waitFor' = [waitFor EXCEPT ![q] = {"T"}]
    /\ pcQ' = [pcQ EXCEPT ![q] = "revoke"]
    /\ UNCHANGED <<caps, arc, freed, pcT, borrow, threads, u>>

\* After the grace period, everything derived from `D0`, if it is still
\* there, is deleted: `S`.
Revoke(q) ==
    /\ pcQ[q] = "revoke" /\ waitFor[q] = {}
    /\ IF "D0" \in caps /\ "S" \in caps
       THEN Delete("S")
       ELSE UNCHANGED <<caps, arc, freed>>
    /\ pcQ' = [pcQ EXCEPT ![q] = "revoked"]
    /\ UNCHANGED <<pcT, borrow, threads, u, revokes, waitFor>>

RevokeEnd(q) ==
    /\ pcQ[q] = "revoked"
    /\ revokes' = revokes - 1
    /\ pcQ' = [pcQ EXCEPT ![q] = "idle"]
    /\ UNCHANGED <<caps, arc, freed, pcT, borrow, threads, u, waitFor>>

CloseD0(q) ==
    /\ pcQ[q] = "idle" /\ "D0" \in caps
    /\ Delete("D0")
    /\ UNCHANGED <<pcT, borrow, threads, u, revokes, pcQ, waitFor>>

\* A start of `U` in `P`: counted among `P`'s threads, held, and a grace
\* period starts.
StartOther(q) ==
    /\ pcQ[q] = "idle" /\ u = "none"
    /\ threads' = 2
    /\ u' = "held"
    /\ waitFor' = [waitFor EXCEPT ![q] = {"T"}]
    /\ pcQ' = [pcQ EXCEPT ![q] = "start"]
    /\ UNCHANGED <<caps, arc, freed, pcT, borrow, revokes>>

\* After the grace period, `U` runs.
Release(q) ==
    /\ pcQ[q] = "start" /\ waitFor[q] = {}
    /\ u' = "run"
    /\ pcQ' = [pcQ EXCEPT ![q] = "idle"]
    /\ UNCHANGED <<caps, arc, freed, pcT, borrow, threads, revokes, waitFor>>

----------------------------------------------------------------------------
(* `U`, `P`'s second thread.                                               *)

UCloseS ==
    /\ u = "run" /\ "S" \in caps
    /\ Delete("S")
    /\ UNCHANGED <<pcT, borrow, threads, u, revokes, pcQ, waitFor>>

UExit ==
    /\ u = "run"
    /\ u' = "gone"
    /\ threads' = 1
    /\ UNCHANGED <<caps, arc, freed, pcT, borrow, revokes, pcQ, waitFor>>

----------------------------------------------------------------------------

Next ==
    \/ Call \/ End \/ Quiescent \/ CloseS \/ StartOwn
    \/ \E q \in Qs :
        \/ RevokeStart(q) \/ Revoke(q) \/ RevokeEnd(q)
        \/ CloseD0(q) \/ StartOther(q) \/ Release(q)
    \/ UCloseS \/ UExit

Spec == Init /\ [][Next]_vars

----------------------------------------------------------------------------

\* The properties.

\* Nothing uses or holds the object once it is freed.
NoUseAfterFree ==
    freed => caps = {} /\ pcT = "idle" /\ borrow = "none"

\* A borrow not counted has its capability in `S`: the store's reference
\* keeps the object.
LightBacked ==
    borrow = "light" => "S" \in caps /\ arc >= 1

\* The object's count is the store's reference and `T`'s counted one, and
\* it is freed when it counts none.
ArcRight ==
    /\ arc = (IF caps # {} THEN 1 ELSE 0) + (IF borrow = "counted" THEN 1 ELSE 0)
    /\ freed <=> arc = 0

=============================================================================
