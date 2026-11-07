\* SPDX-FileCopyrightText: 2026 The Dukovo contributors
\* SPDX-License-Identifier: Apache-2.0 OR MIT

------------------------------ MODULE MountRefs ------------------------------
(***************************************************************************)
(* A model of how a mount's references are counted (ADR 0404): on each   *)
(* CPU while the mount is in the tree, by its `Arc` otherwise              *)
(* (kernel/lib/sync/src/cpu_ref.rs), and how `unmount` adds them up        *)
(* (kernel/fs/vfs/src/mount.rs).                                           *)
(*                                                                         *)
(* One mount, in the tree, with a line of counts, one on each CPU. The     *)
(* table holds a reference to it, counted on a CPU, and the counts hold    *)
(* one of the `Arc`'s: its `group`. Threads run on CPUs and move between   *)
(* them outside read sections. A thread gets a reference from the table,  *)
(* under the table's lock, or clones one it holds; it drops them, maybe on *)
(* another CPU than made them.                                             *)
(*                                                                         *)
(* A clone or a drop of a reference counted on the CPUs starts a read      *)
(* section and loads the mount's state. In the tree, it adds one to, or    *)
(* takes one from, its CPU's count and ends the section. Otherwise a clone *)
(* adds one to the `Arc`'s count; a drop takes the slow lock, takes one    *)
(* from its CPU's count and, if the mount has left the tree, adds the      *)
(* counts up: at none it frees the mount's line and drops the group's      *)
(* reference. A reference counted by the `Arc` is dropped as an `Arc` is:  *)
(* the last one frees the mount.                                           *)
(*                                                                         *)
(* The unmounting thread `U` holds a reference of its own. It takes the    *)
(* table's lock, pauses the counting on the CPUs and waits for a grace     *)
(* period (every read section that had started has ended). Under the slow  *)
(* lock it adds the counts up, one CPU at a time, and the `Arc`'s. If more *)
(* than the table and itself hold the mount, it goes back to counting on   *)
(* the CPUs; otherwise the mount leaves the table, whose reference it      *)
(* drops, and the tree: under the slow lock, its state becomes gone, and   *)
(* if the counts add up to none the line is freed then.                    *)
(*                                                                         *)
(* Counts wrap at `Wrap`, as the kernel's 32-bit counts do; what they add  *)
(* up to is always less. Each change of a count is one step: only its own  *)
(* CPU changes a count, which the kernel holds, with interrupts off, for   *)
(* the plain load and store that change it (ADR 0406).                     *)
(*                                                                         *)
(* NoUseAfterFree says nothing uses the mount once it is freed;            *)
(* UnmountSeesAll says it leaves the table only when nothing but `U`       *)
(* holds it; CountsRight and ArcRight say the counts on the CPUs and the   *)
(* `Arc`'s are the references there are; CleanLine says a freed line is   *)
(* all zero for the next mount. Leaving out the grace period breaks        *)
(* UnmountSeesAll: a clone that loaded "tree" adds to a count already      *)
(* added up.                                                               *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    Threads,    \* threads that hold references; `U` among them
    U,          \* the thread that unmounts
    CPUs,       \* the CPUs
    Wrap,       \* where a count wraps
    Max,        \* the most references a thread holds
    None        \* no thread

VARIABLES
    state,      \* the mount's: "tree", "paused" or "gone"
    count,      \* each CPU's count of the mount's references, mod Wrap
    arc,        \* the `Arc`'s count
    group,      \* whether the counts hold a reference of the `Arc`'s
    line,       \* whether the mount has its line of counts
    freed,      \* whether the mount is freed
    inTable,    \* whether the table holds it, by a reference on the CPUs
    tableLock,  \* the table lock's holder, or None
    slow,       \* the slow lock's holder, or None
    cpuRefs,    \* each thread's references counted on the CPUs
    arcRefs,    \* each thread's references counted by the `Arc`
    on,         \* the CPU each thread runs on
    inRead,     \* the threads in a read section
    waitFor,    \* the read sections `U`'s grace period waits for
    pc,         \* where each thread is
    after,      \* where a thread goes on after a drop: "idle" or "leave"
    sum,        \* `U`'s sum of the counts so far
    added       \* the CPUs whose count `U` has added

vars == <<state, count, arc, group, line, freed, inTable, tableLock, slow,
          cpuRefs, arcRefs, on, inRead, waitFor, pc, after, sum, added>>

Steps == {"idle", "clone", "cloneFast", "cloneArc", "drop", "dropFast",
          "dropSlow", "dropGroup", "sync", "add", "leave", "leaveGroup"}

\* The steps at which a thread touches the mount.
Touching == Steps \ {"idle"}

Held(t) == cpuRefs[t] + arcRefs[t]

\* The sum of `f`'s values over `xs`.
RECURSIVE SumOver(_, _)
SumOver(f, xs) ==
    IF xs = {} THEN 0
    ELSE LET x == CHOOSE x \in xs : TRUE IN f[x] + SumOver(f, xs \ {x})

\* What counts like `c` add up to, wrapping.
Counted(c) == SumOver(c, CPUs) % Wrap

Inc(n) == (n + 1) % Wrap
Dec(n) == (n + Wrap - 1) % Wrap

TypeOK ==
    /\ state \in {"tree", "paused", "gone"}
    /\ count \in [CPUs -> 0..(Wrap - 1)]
    /\ arc \in Nat
    /\ group \in BOOLEAN
    /\ line \in BOOLEAN
    /\ freed \in BOOLEAN
    /\ inTable \in BOOLEAN
    /\ tableLock \in Threads \union {None}
    /\ slow \in Threads \union {None}
    /\ cpuRefs \in [Threads -> 0..(Max + 1)]
    /\ arcRefs \in [Threads -> 0..Max]
    /\ on \in [Threads -> CPUs]
    /\ inRead \subseteq Threads
    /\ waitFor \subseteq Threads
    /\ pc \in [Threads -> Steps]
    /\ after \in [Threads -> {"idle", "leave"}]
    /\ sum \in 0..(Wrap - 1)
    /\ added \subseteq CPUs

\* The mount in the tree: the table's reference counted on the first CPU,
\* and the group's the only one of the `Arc`'s.
Init ==
    /\ state = "tree"
    /\ count = [c \in CPUs |-> IF c = CHOOSE c1 \in CPUs : TRUE THEN 1 ELSE 0]
    /\ arc = 1
    /\ group = TRUE
    /\ line = TRUE
    /\ freed = FALSE
    /\ inTable = TRUE
    /\ tableLock = None
    /\ slow = None
    /\ cpuRefs = [t \in Threads |-> 0]
    /\ arcRefs = [t \in Threads |-> 0]
    /\ on \in [Threads -> CPUs]
    /\ inRead = {}
    /\ waitFor = {}
    /\ pc = [t \in Threads |-> "idle"]
    /\ after = [t \in Threads |-> "idle"]
    /\ sum = 0
    /\ added = {}

\* Leaving a read section also lets a grace period that waits for it on.
EndRead(t) ==
    /\ inRead' = inRead \ {t}
    /\ waitFor' = waitFor \ {t}

----------------------------------------------------------------------------
(* Making a reference.                                                     *)

\* From the table, under its lock, as a walk finds a mount on a directory.
FromTable(t) ==
    /\ pc[t] = "idle" /\ Held(t) < Max
    /\ inTable /\ tableLock = None
    /\ tableLock' = t
    /\ pc' = [pc EXCEPT ![t] = "clone"]
    /\ UNCHANGED <<state, count, arc, group, line, freed, inTable, slow,
                   cpuRefs, arcRefs, on, inRead, waitFor, after, sum, added>>

\* From one it holds, as a walk takes its base's.
FromHeld(t) ==
    /\ pc[t] = "idle" /\ Held(t) > 0 /\ Held(t) < Max
    /\ pc' = [pc EXCEPT ![t] = "clone"]
    /\ UNCHANGED <<state, count, arc, group, line, freed, inTable, tableLock,
                   slow, cpuRefs, arcRefs, on, inRead, waitFor, after, sum,
                   added>>

\* A read section, and the mount's state.
Clone(t) ==
    /\ pc[t] = "clone"
    /\ inRead' = inRead \union {t}
    /\ pc' = [pc EXCEPT ![t] = IF state = "tree" THEN "cloneFast" ELSE "cloneArc"]
    /\ UNCHANGED <<state, count, arc, group, line, freed, inTable, tableLock,
                   slow, cpuRefs, arcRefs, on, waitFor, after, sum, added>>

\* The table's lock, if the reference came from it, goes with the step
\* that ends the clone.
Unlocked(t) == tableLock' = IF tableLock = t THEN None ELSE tableLock

CloneFast(t) ==
    /\ pc[t] = "cloneFast"
    /\ count' = [count EXCEPT ![on[t]] = Inc(@)]
    /\ cpuRefs' = [cpuRefs EXCEPT ![t] = @ + 1]
    /\ EndRead(t)
    /\ Unlocked(t)
    /\ pc' = [pc EXCEPT ![t] = "idle"]
    /\ UNCHANGED <<state, arc, group, line, freed, inTable, slow, arcRefs, on,
                   after, sum, added>>

CloneArc(t) ==
    /\ pc[t] = "cloneArc"
    /\ arc' = arc + 1
    /\ arcRefs' = [arcRefs EXCEPT ![t] = @ + 1]
    /\ EndRead(t)
    /\ Unlocked(t)
    /\ pc' = [pc EXCEPT ![t] = "idle"]
    /\ UNCHANGED <<state, count, group, line, freed, inTable, slow, cpuRefs,
                   on, after, sum, added>>

----------------------------------------------------------------------------
(* Dropping a reference.                                                   *)

\* One counted by the `Arc`: the last frees the mount.
DropArc(t) ==
    /\ pc[t] = "idle" /\ arcRefs[t] > 0
    /\ arcRefs' = [arcRefs EXCEPT ![t] = @ - 1]
    /\ arc' = arc - 1
    /\ freed' = (arc = 1)
    /\ UNCHANGED <<state, count, group, line, inTable, tableLock, slow,
                   cpuRefs, on, inRead, waitFor, pc, after, sum, added>>

\* One counted on the CPUs: a read section, and the mount's state.
DropCpu(t) ==
    /\ pc[t] = "idle" /\ cpuRefs[t] > 0
    /\ pc' = [pc EXCEPT ![t] = "drop"]
    /\ UNCHANGED <<state, count, arc, group, line, freed, inTable, tableLock,
                   slow, cpuRefs, arcRefs, on, inRead, waitFor, after, sum,
                   added>>

Drop(t) ==
    /\ pc[t] = "drop"
    /\ inRead' = inRead \union {t}
    /\ pc' = [pc EXCEPT ![t] = IF state = "tree" THEN "dropFast" ELSE "dropSlow"]
    /\ UNCHANGED <<state, count, arc, group, line, freed, inTable, tableLock,
                   slow, cpuRefs, arcRefs, on, waitFor, after, sum, added>>

DropFast(t) ==
    /\ pc[t] = "dropFast"
    /\ count' = [count EXCEPT ![on[t]] = Dec(@)]
    /\ cpuRefs' = [cpuRefs EXCEPT ![t] = @ - 1]
    /\ EndRead(t)
    /\ pc' = [pc EXCEPT ![t] = after[t]]
    /\ after' = [after EXCEPT ![t] = "idle"]
    /\ UNCHANGED <<state, arc, group, line, freed, inTable, tableLock, slow,
                   arcRefs, on, sum, added>>

\* The read section ends, then the slow lock: taken and let go in one step,
\* with what is done under it.
DropSlow(t) ==
    /\ pc[t] = "dropSlow"
    /\ slow = None
    /\ LET counted == [count EXCEPT ![on[t]] = Dec(@)]
           last == state = "gone" /\ line /\ Counted(counted) = 0
       IN /\ count' = IF last THEN [c \in CPUs |-> 0] ELSE counted
          /\ line' = (line /\ ~last)
          /\ IF last
             THEN /\ pc' = [pc EXCEPT ![t] = "dropGroup"]
                  /\ UNCHANGED after
             ELSE /\ pc' = [pc EXCEPT ![t] = after[t]]
                  /\ after' = [after EXCEPT ![t] = "idle"]
    /\ cpuRefs' = [cpuRefs EXCEPT ![t] = @ - 1]
    /\ EndRead(t)
    /\ UNCHANGED <<state, arc, group, freed, inTable, tableLock, slow, arcRefs,
                   on, sum, added>>

\* The counts' reference of the `Arc`, after the slow lock.
DropGroup(t) ==
    /\ pc[t] = "dropGroup"
    /\ group' = FALSE
    /\ arc' = arc - 1
    /\ freed' = (arc = 1)
    /\ pc' = [pc EXCEPT ![t] = after[t]]
    /\ after' = [after EXCEPT ![t] = "idle"]
    /\ UNCHANGED <<state, count, line, inTable, tableLock, slow, cpuRefs,
                   arcRefs, on, inRead, waitFor, sum, added>>

\* A thread moves to another CPU, outside read sections.
Move(t) ==
    /\ t \notin inRead
    /\ \E c \in CPUs : on' = [on EXCEPT ![t] = c]
    /\ UNCHANGED <<state, count, arc, group, line, freed, inTable, tableLock,
                   slow, cpuRefs, arcRefs, inRead, waitFor, pc, after, sum,
                   added>>

----------------------------------------------------------------------------
(* `unmount`, by `U`, which holds one reference of its own.                *)

\* The table's lock, and the counting on the CPUs paused; the grace
\* period waits for every read section running now.
Pause ==
    /\ pc[U] = "idle" /\ Held(U) = 1
    /\ inTable /\ tableLock = None
    /\ tableLock' = U
    /\ state' = "paused"
    /\ waitFor' = inRead
    /\ pc' = [pc EXCEPT ![U] = "sync"]
    /\ UNCHANGED <<count, arc, group, line, freed, inTable, slow, cpuRefs,
                   arcRefs, on, inRead, after, sum, added>>

\* `rcu::synchronize`, then the slow lock.
Sync ==
    /\ pc[U] = "sync" /\ waitFor = {} /\ slow = None
    /\ slow' = U
    /\ sum' = 0
    /\ added' = {}
    /\ pc' = [pc EXCEPT ![U] = "add"]
    /\ UNCHANGED <<state, count, arc, group, line, freed, inTable, tableLock,
                   cpuRefs, arcRefs, on, inRead, waitFor, after>>

\* One CPU's count at a time.
Add ==
    /\ pc[U] = "add" /\ added # CPUs
    /\ \E c \in CPUs \ added :
        /\ sum' = (sum + count[c]) % Wrap
        /\ added' = added \union {c}
    /\ UNCHANGED <<state, count, arc, group, line, freed, inTable, tableLock,
                   slow, cpuRefs, arcRefs, on, inRead, waitFor, pc, after>>

\* The references: the counts' and the `Arc`'s but the group's. More than
\* the table's and `U`'s is a mount in use, which goes back to counting
\* on the CPUs; otherwise it leaves the table, and `U` drops the table's
\* reference.
Decide ==
    /\ pc[U] = "add" /\ added = CPUs
    /\ slow' = None
    /\ IF sum + arc - (IF group THEN 1 ELSE 0) > 2
       THEN /\ state' = "tree"
            /\ tableLock' = None
            /\ pc' = [pc EXCEPT ![U] = "idle"]
            /\ UNCHANGED <<inTable, cpuRefs, after>>
       ELSE /\ inTable' = FALSE
            /\ cpuRefs' = [cpuRefs EXCEPT ![U] = @ + 1]
            /\ after' = [after EXCEPT ![U] = "leave"]
            /\ pc' = [pc EXCEPT ![U] = "drop"]
            /\ UNCHANGED <<state, tableLock>>
    /\ UNCHANGED <<count, arc, group, line, freed, arcRefs, on, inRead,
                   waitFor, sum, added>>

\* The mount leaves the tree, under the slow lock: its line goes now if
\* the counts add up to none; then the table's lock.
Leave ==
    /\ pc[U] = "leave" /\ slow = None
    /\ state' = "gone"
    /\ LET last == Counted(count) = 0 /\ line
       IN /\ count' = IF last THEN [c \in CPUs |-> 0] ELSE count
          /\ line' = (line /\ ~last)
          /\ pc' = [pc EXCEPT ![U] = IF last THEN "leaveGroup" ELSE "idle"]
    /\ tableLock' = None
    /\ UNCHANGED <<arc, group, freed, inTable, slow, cpuRefs, arcRefs, on,
                   inRead, waitFor, after, sum, added>>

LeaveGroup ==
    /\ pc[U] = "leaveGroup"
    /\ group' = FALSE
    /\ arc' = arc - 1
    /\ freed' = (arc = 1)
    /\ pc' = [pc EXCEPT ![U] = "idle"]
    /\ UNCHANGED <<state, count, line, inTable, tableLock, slow, cpuRefs,
                   arcRefs, on, inRead, waitFor, after, sum, added>>

----------------------------------------------------------------------------

Thread(t) ==
    \/ FromTable(t) \/ FromHeld(t) \/ Clone(t) \/ CloneFast(t)
    \/ CloneArc(t) \/ DropArc(t) \/ DropCpu(t) \/ Drop(t) \/ DropFast(t)
    \/ DropSlow(t) \/ DropGroup(t) \/ Move(t)

Next ==
    \/ \E t \in Threads : Thread(t)
    \/ Pause \/ Sync \/ Add \/ Decide \/ Leave \/ LeaveGroup

Spec == Init /\ [][Next]_vars

----------------------------------------------------------------------------

\* The properties.

ArcRefs == SumOver(arcRefs, Threads)

\* Nothing touches a freed mount, and nothing holds it.
NoUseAfterFree ==
    freed =>
        /\ ~inTable /\ ~group /\ ~line
        /\ \A t \in Threads : Held(t) = 0 /\ pc[t] \notin Touching

\* It leaves the table only when nothing but `U` holds it: no thread can
\* reach it after.
UnmountSeesAll ==
    ~inTable => \A t \in Threads \ {U} : Held(t) = 0 /\ pc[t] = "idle"

\* The counts on the CPUs add up to the references counted on them: the
\* threads' and the table's.
CountsRight ==
    Counted(count) = (SumOver(cpuRefs, Threads) + IF inTable THEN 1 ELSE 0) % Wrap

\* The `Arc` counts its own references and the group's, and the mount is
\* freed when it counts none.
ArcRight ==
    /\ arc = ArcRefs + IF group THEN 1 ELSE 0
    /\ freed <=> arc = 0

\* The counts hold the group's reference while they have a line; a line
\* given back is all zero, for the next mount, and stays so.
CleanLine ==
    /\ line => group
    /\ ~line => \A c \in CPUs : count[c] = 0

=============================================================================
