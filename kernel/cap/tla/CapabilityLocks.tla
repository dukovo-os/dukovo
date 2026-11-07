\* SPDX-FileCopyrightText: 2026 The Dukovo contributors
\* SPDX-License-Identifier: Apache-2.0 OR MIT

--------------------------- MODULE CapabilityLocks ---------------------------
(***************************************************************************)
(* The capability store's locks (ADR 0369): threads change the store at   *)
(* once, each holding only the locks its change needs.                     *)
(*                                                                         *)
(* Each capability space has a lock of its own, which guards what its     *)
(* slots hold: the capability, and a `linked` flag saying the slot may     *)
(* have a parent or children. One tree lock guards the links between       *)
(* slots, which cross spaces. A slot whose flag is clear is in no tree:    *)
(* no link leads to it or from it. So putting a new object's capability    *)
(* in an empty slot, and deleting or moving a capability whose slot is     *)
(* not linked, touch that slot alone: they take only its space's lock (the *)
(* fast path). Deriving, deleting or moving a linked capability, and       *)
(* revoking, take the tree lock and then the locks of the spaces whose     *)
(* slots they fill or empty (the slow path). A fast path that finds its    *)
(* slot linked lets go and starts again on the slow path.                  *)
(*                                                                         *)
(* The rules: the tree lock comes before any space's lock, space locks are *)
(* taken lowest space first, and the flag is set or cleared only with the  *)
(* slot's space locked. So a fast path that read the flag clear, with the  *)
(* space locked, finds the slot unlinked when it acts: nobody can derive   *)
(* from it or into it meanwhile, as that needs the same lock.              *)
(*                                                                         *)
(* The store's operations are those of Capabilities.tla, taken from it.   *)
(* The slow path does each as one step with its locks held, as the store   *)
(* does today. The fast path does what the kernel's will do: it changes    *)
(* only its own slots, and leaves every link alone. If the rules let a     *)
(* fast path act on a slot with relatives, a link would be left dangling,  *)
(* and Capabilities' invariants would fail. A revoke goes in turns, each   *)
(* under the tree lock: it marks its capability, looks at the mark again   *)
(* and clears it with the capability's space locked, and deletes each leaf *)
(* with the leaf's space locked. Between turns it lets go of every lock.   *)
(*                                                                         *)
(* Rights, badges and retyping are those of the slow path's derive, so the *)
(* model derives with every right and no badge.                            *)
(*                                                                         *)
(* Each object has a record in the object table that counts the            *)
(* capabilities naming it; a record whose count is 0 is free, and the      *)
(* object it named is gone (ADR 0405). Each record has a lock of its own,  *)
(* taken last and only around one change of its count, so each change is  *)
(* one step here. The slow path changes counts inside its step, with its   *)
(* locks held. The fast path changes them outside its space's lock: a      *)
(* create makes its object's record before it takes the lock, and gives    *)
(* the record back after it if the slot was full; a delete gives back its  *)
(* capability's count after the lock has gone. So a count may stand above  *)
(* the capabilities naming its object while an operation is under way,     *)
(* never below them, and a record is free only once nothing names its      *)
(* object.                                                                 *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS
    Slots,      \* the slots of every capability space
    Objects,    \* names for kernel objects
    Rights,     \* the rights; must include "copy"
    MaxDepth,   \* the deepest a capability can be derived (MAX_DEPTH)
    None,       \* no parent, no lock holder
    NoCap,      \* an empty slot
    Threads,    \* the threads changing the store
    Low         \* the slots of space 1; the others are space 2's

ASSUME Low \subseteq Slots /\ None \notin Threads

VARIABLES
    cap,        \* what each slot holds              (as in Capabilities)
    parent,     \* each capability's parent          (as in Capabilities)
    depth,      \* each capability's stored depth    (as in Capabilities)
    revoking,   \* the revoke marks                  (as in Capabilities)
    origin,     \* what each object was retyped from (as in Capabilities)
    src,        \* ghost: what each was derived from (as in Capabilities)
    linked,     \* each slot's flag: it may have a parent or children
    tree,       \* the thread holding the tree lock, or None
    held,       \* the thread holding each space's lock, or None
    pc,         \* where each thread is
    op,         \* what each thread is doing
    count       \* each object's record: the capabilities it counts, 0 = free

store == <<cap, parent, depth, revoking, origin, src>>
vars == <<cap, parent, depth, revoking, origin, src, linked, tree, held, pc, op, count>>

C == INSTANCE Capabilities

Spaces == {1, 2}
SpaceOf(s) == IF s \in Low THEN 1 ELSE 2

\* A slot in no tree: no parent, no children (so no siblings either).
Unlinked(s) == parent[s] = None /\ \A x \in Slots : parent[x] # s

\* What a thread does. `a` is the slot it works on, `b` the other one
\* (where a move or a derive puts the capability, the leaf a revoke
\* deletes), `o` the object a create makes, `slow` says whether it is on
\* the slow path, `begun` whether a revoke has set its mark, and `fin` the
\* record whose count the fast path gives back once its lock has gone
\* (None for none).
Kinds == {"create", "delete", "move", "derive", "revoke"}
Ops == [kind : Kinds, a : Slots, b : Slots, o : Objects, slow : BOOLEAN, begun : BOOLEAN,
        fin : Objects \cup {None}]

\* The spaces whose locks an operation takes: those of the slots it fills
\* or empties. A revoke takes them one at a time, as it goes.
Needed(o) ==
    CASE o.kind = "create" -> {SpaceOf(o.a)}
      [] o.kind = "delete" -> {SpaceOf(o.a)}
      [] o.kind \in {"move", "derive"} -> {SpaceOf(o.a), SpaceOf(o.b)}
      [] o.kind = "revoke" -> {}

\* What an idle thread's `op` holds, so that idle threads differ in
\* nothing else.
Nothing == [kind |-> "create", a |-> CHOOSE s \in Slots : TRUE,
            b |-> CHOOSE s \in Slots : TRUE, o |-> CHOOSE o \in Objects : TRUE,
            slow |-> FALSE, begun |-> FALSE, fin |-> None]

\* The capabilities naming `o`.
Named(o) == Cardinality({x \in C!Full : cap[x].obj = o})

HeldBy(t) == {x \in Spaces : held[x] = t}

Least(set) == CHOOSE x \in set : \A y \in set : x <= y

Steps == {"idle", "make", "tree", "spaces", "check", "commit", "release",
          "finish", "revoke", "mark", "again", "down", "leaf"}

TypeOK ==
    /\ C!TypeOK
    /\ linked \in [Slots -> BOOLEAN]
    /\ tree \in Threads \cup {None}
    /\ held \in [Spaces -> Threads \cup {None}]
    /\ pc \in [Threads -> Steps]
    /\ op \in [Threads -> Ops]
    /\ count \in [Objects -> Nat]

Init ==
    /\ C!Init
    /\ linked = [s \in Slots |-> FALSE]
    /\ tree = None
    /\ held = [x \in Spaces |-> None]
    /\ pc = [t \in Threads |-> "idle"]
    /\ op = [t \in Threads |-> Nothing]
    /\ count = [o \in Objects |-> 0]

-----------------------------------------------------------------------------
(* Taking and letting go of locks.                                         *)

\* A thread picks what to do next, from what it sees of the store with no
\* lock held (which may change before it acts). Creating, deleting and
\* moving try the fast path; deriving and revoking go straight to the slow
\* one. A create first makes its object's record.
Start(t) ==
    /\ pc[t] = "idle"
    /\ \E k \in Kinds, a \in Slots, b \in Slots :
          /\ (k = "create") = (a \notin C!Full)
          /\ k \in {"move", "derive"} => a # b /\ b \notin C!Full
          /\ k \notin {"move", "derive"} => b = a
          /\ op' = [op EXCEPT ![t] = [kind |-> k, a |-> a, b |-> b,
                                      o |-> CHOOSE x \in Objects : TRUE,
                                      slow |-> k \in {"derive", "revoke"},
                                      begun |-> FALSE, fin |-> None]]
          /\ pc' = [pc EXCEPT ![t] = CASE k \in {"derive", "revoke"} -> "tree"
                                        [] k = "create" -> "make"
                                        [] OTHER -> "spaces"]
    /\ UNCHANGED <<store, linked, tree, held, count>>

\* A create takes a free record, with no lock but the record's, and counts
\* the capability it will place: nobody else can take the record now, and
\* nothing names its object. With no record free it gives up (the kernel
\* makes a new record then, or answers that memory ran out).
Make(t) ==
    /\ pc[t] = "make"
    /\ IF \E o \in Objects : count[o] = 0
       THEN \E o \in Objects :
               /\ count[o] = 0
               /\ count' = [count EXCEPT ![o] = 1]
               /\ op' = [op EXCEPT ![t].o = o]
               /\ pc' = [pc EXCEPT ![t] = "spaces"]
       ELSE /\ pc' = [pc EXCEPT ![t] = "idle"]
            /\ op' = [op EXCEPT ![t] = Nothing]
            /\ UNCHANGED count
    /\ UNCHANGED <<store, linked, tree, held>>

\* The tree lock, holding no space's lock (`LockOrder`).
TakeTree(t) ==
    /\ pc[t] = "tree"
    /\ tree = None
    /\ tree' = t
    /\ pc' = [pc EXCEPT ![t] = IF op[t].kind = "revoke" THEN "revoke" ELSE "spaces"]
    /\ UNCHANGED <<store, linked, held, op, count>>

\* The spaces' locks, lowest first. Then the fast path checks its slot,
\* and the slow path acts.
TakeSpaces(t) ==
    /\ pc[t] = "spaces"
    /\ LET want == Needed(op[t]) \ HeldBy(t)
       IN  IF want = {}
           THEN /\ pc' = [pc EXCEPT ![t] = IF op[t].slow \/ op[t].kind = "create"
                                           THEN "commit" ELSE "check"]
                /\ UNCHANGED held
           ELSE /\ held[Least(want)] = None
                /\ held' = [held EXCEPT ![Least(want)] = t]
                /\ UNCHANGED pc
    /\ UNCHANGED <<store, linked, tree, op, count>>

\* Every lock the thread holds goes at once (the order of letting go
\* does not matter). A fast path with a record to give back does that
\* next.
Release(t) ==
    /\ pc[t] = "release"
    /\ tree' = IF tree = t THEN None ELSE tree
    /\ held' = [x \in Spaces |-> IF held[x] = t THEN None ELSE held[x]]
    /\ IF op[t].fin = None
       THEN /\ pc' = [pc EXCEPT ![t] = "idle"]
            /\ op' = [op EXCEPT ![t] = Nothing]
       ELSE /\ pc' = [pc EXCEPT ![t] = "finish"]
            /\ UNCHANGED op
    /\ UNCHANGED <<store, linked, count>>

\* With no lock but the record's: the count the fast path took out of a
\* slot (a delete), or counted for a slot it found full (a create), goes.
\* A record whose count reaches 0 is free.
Finish(t) ==
    /\ pc[t] = "finish"
    /\ count' = [count EXCEPT ![op[t].fin] = @ - 1]
    /\ pc' = [pc EXCEPT ![t] = "idle"]
    /\ op' = [op EXCEPT ![t] = Nothing]
    /\ UNCHANGED <<store, linked, tree, held>>

-----------------------------------------------------------------------------
(* The fast path: its space's lock only.                                   *)

\* With the space locked: a slot that may have relatives sends the
\* operation to the slow path, letting go of the space first.
Check(t) ==
    /\ pc[t] = "check"
    /\ LET o == op[t]
           fails == \/ o.a \notin C!Full
                    \/ o.kind = "move" /\ (o.b \in C!Full \/ revoking[o.a])
       IN  CASE fails ->
                    /\ pc' = [pc EXCEPT ![t] = "release"]
                    /\ UNCHANGED <<held, op>>
             [] linked[o.a] ->
                    /\ held' = [x \in Spaces |-> IF held[x] = t THEN None ELSE held[x]]
                    /\ op' = [op EXCEPT ![t].slow = TRUE]
                    /\ pc' = [pc EXCEPT ![t] = "tree"]
             [] OTHER ->
                    /\ pc' = [pc EXCEPT ![t] = "commit"]
                    /\ UNCHANGED <<held, op>>
    /\ UNCHANGED <<store, linked, tree, count>>

\* The new object's original capability, in an empty slot, with no
\* relatives: the store's create, which links nothing. Its record was
\* made before the lock, and counts it already. A slot filled meanwhile
\* leaves the record to be given back once the lock has gone.
FastCreate(t, o) ==
    IF o.a \notin C!Full
    THEN /\ cap' = [cap EXCEPT ![o.a] = [obj |-> o.o, rights |-> Rights, badge |-> 0]]
         /\ origin' = [origin EXCEPT ![o.o] = None]
         /\ UNCHANGED <<parent, depth, revoking, src, linked, op>>
    ELSE /\ op' = [op EXCEPT ![t].fin = o.o]
         /\ UNCHANGED <<store, linked>>

\* Empties the slot and touches nothing else: no link is followed or
\* changed, and the capability's count is given back once the lock has
\* gone.
FastDelete(t, o) ==
    /\ cap' = [cap EXCEPT ![o.a] = NoCap]
    /\ depth' = [depth EXCEPT ![o.a] = 0]
    /\ revoking' = [revoking EXCEPT ![o.a] = FALSE]
    /\ src' = [src EXCEPT ![o.a] = {}]
    /\ op' = [op EXCEPT ![t].fin = cap[o.a].obj]
    /\ UNCHANGED <<parent, origin, linked>>

\* Carries the capability and its depth to the other slot, which may be
\* in the other space, and touches nothing else.
FastMove(o) ==
    /\ cap' = [cap EXCEPT ![o.b] = cap[o.a], ![o.a] = NoCap]
    /\ depth' = [depth EXCEPT ![o.b] = depth[o.a], ![o.a] = 0]
    /\ src' = [src EXCEPT ![o.b] = src[o.a], ![o.a] = {}]
    /\ UNCHANGED <<parent, revoking, origin, linked, count>>

-----------------------------------------------------------------------------
(* The slow path: the tree lock, then the spaces' locks.                   *)

\* After a delete, a parent in the same space (whose lock is held) that is
\* left with no relatives gets its flag cleared: the common case of a copy
\* made in a process and deleted again. A parent in another space keeps
\* its flag, which is safe: a set flag only sends a fast path to the slow
\* one.
ClearParent(s) ==
    LET p == parent[s]
    IN  IF p # None /\ SpaceOf(p) = SpaceOf(s)
           /\ parent'[p] = None /\ \A x \in Slots : parent'[x] # p
        THEN linked' = [linked EXCEPT ![s] = FALSE, ![p] = FALSE]
        ELSE linked' = [linked EXCEPT ![s] = FALSE]

DeriveOK(f, s) ==
    /\ f \in C!Full
    /\ s \notin C!Full
    /\ "copy" \in cap[f].rights
    /\ C!Distance(f) < MaxDepth
    /\ ~C!Blocked(f)

\* The slow path's delete: the capability's count goes in the same step.
SlowRemove(s) ==
    /\ C!Remove(s)
    /\ ClearParent(s)
    /\ count' = [count EXCEPT ![cap[s].obj] = @ - 1]

SlowStep(o) ==
    CASE o.kind = "delete" ->
            IF o.a \in C!Full THEN SlowRemove(o.a)
            ELSE UNCHANGED <<store, linked, count>>
      [] o.kind = "move" ->
            IF o.a \in C!Full /\ o.b \notin C!Full /\ ~revoking[o.a]
            THEN /\ C!Move(o.a, o.b)
                 /\ linked' = [linked EXCEPT ![o.b] = linked[o.a], ![o.a] = FALSE]
                 /\ UNCHANGED count
            ELSE UNCHANGED <<store, linked, count>>
      [] o.kind = "derive" ->
            IF DeriveOK(o.a, o.b)
            THEN /\ C!Derive(o.a, o.b, Rights, 0)
                 /\ linked' = [linked EXCEPT ![o.a] = TRUE, ![o.b] = TRUE]
                 /\ count' = [count EXCEPT ![cap[o.a].obj] = @ + 1]
            ELSE UNCHANGED <<store, linked, count>>

\* The step itself, with every lock it needs held.
Commit(t) ==
    /\ pc[t] = "commit"
    /\ LET o == op[t]
       IN  CASE o.kind = "create" -> FastCreate(t, o) /\ UNCHANGED count
             [] o.slow -> SlowStep(o) /\ UNCHANGED op
             [] o.kind = "delete" -> FastDelete(t, o) /\ UNCHANGED count
             [] o.kind = "move" -> FastMove(o) /\ UNCHANGED op
    /\ pc' = [pc EXCEPT ![t] = "release"]
    /\ UNCHANGED <<tree, held>>

\* A revoke holds the tree lock through each turn and one space's lock at
\* a time: the capability's, to set the mark, to look at it again on each
\* later turn, and to clear it; or a leaf's, to delete it. The kernel's
\* revoke deletes up to 64 leaves a turn; here a turn is one step, and any
\* number of other threads' steps come between turns.
Revoke(t) ==
    /\ pc[t] = "revoke"
    /\ LET x == SpaceOf(op[t].a)
       IN  /\ held[x] = None
           /\ held' = [held EXCEPT ![x] = t]
    /\ pc' = [pc EXCEPT ![t] = IF op[t].begun THEN "again" ELSE "mark"]
    /\ UNCHANGED <<store, linked, tree, op, count>>

\* Lets go of everything between turns; the next turn starts with the
\* tree lock again.
NextTurn(t) ==
    /\ tree' = None
    /\ held' = [x \in Spaces |-> IF held[x] = t THEN None ELSE held[x]]
    /\ pc' = [pc EXCEPT ![t] = "tree"]

\* The first turn marks the capability (joining a revoke of it already
\* under way, as the store does). An empty slot has nothing to revoke.
Mark(t) ==
    /\ pc[t] = "mark"
    /\ LET s == op[t].a
       IN  IF s \in C!Full
           THEN /\ revoking' = [revoking EXCEPT ![s] = TRUE]
                /\ UNCHANGED <<cap, parent, depth, origin, src>>
                /\ op' = [op EXCEPT ![t].begun = TRUE]
                /\ NextTurn(t)
           ELSE /\ pc' = [pc EXCEPT ![t] = "release"]
                /\ UNCHANGED <<store, tree, held, op>>
    /\ UNCHANGED <<linked, count>>

\* Each later turn looks at the mark again, as `continue_revoke` does.
\* Without it (another revoke of the capability finished, or the
\* capability was deleted, perhaps with another put in its slot), what
\* was asked is done. With nothing left below the capability, the mark is
\* cleared, and so is the capability's flag if it has no parent either.
\* Otherwise the turn lets go of the capability's space and goes down to
\* a leaf.
Again(t) ==
    /\ pc[t] = "again"
    /\ LET s == op[t].a
       IN  CASE ~revoking[s] ->
                    /\ pc' = [pc EXCEPT ![t] = "release"]
                    /\ UNCHANGED <<store, linked, held>>
             [] C!Descendants(s) = {} ->
                    /\ C!RevokeDone(s)
                    /\ linked' = IF parent[s] = None
                                 THEN [linked EXCEPT ![s] = FALSE] ELSE linked
                    /\ pc' = [pc EXCEPT ![t] = "release"]
                    /\ UNCHANGED held
             [] OTHER ->
                    /\ held' = [x \in Spaces |-> IF held[x] = t THEN None ELSE held[x]]
                    /\ pc' = [pc EXCEPT ![t] = "down"]
                    /\ UNCHANGED <<store, linked>>
    /\ UNCHANGED <<tree, op, count>>

\* The walk down the tree follows links, under the tree lock alone; the
\* leaf it reaches is deleted with the leaf's space locked.
Down(t) ==
    /\ pc[t] = "down"
    /\ \E l \in C!Descendants(op[t].a) :
          /\ C!Descendants(l) = {}
          /\ held[SpaceOf(l)] = None
          /\ held' = [held EXCEPT ![SpaceOf(l)] = t]
          /\ op' = [op EXCEPT ![t].b = l]
          /\ pc' = [pc EXCEPT ![t] = "leaf"]
    /\ UNCHANGED <<store, linked, tree, count>>

Leaf(t) ==
    /\ pc[t] = "leaf"
    /\ SlowRemove(op[t].b)
    /\ NextTurn(t)
    /\ UNCHANGED op

-----------------------------------------------------------------------------

Thread(t) ==
    \/ Start(t) \/ Make(t) \/ TakeTree(t) \/ TakeSpaces(t) \/ Check(t)
    \/ Commit(t) \/ Release(t) \/ Finish(t) \/ Revoke(t) \/ Mark(t)
    \/ Again(t) \/ Down(t) \/ Leaf(t)

Next == \E t \in Threads : Thread(t)

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* Invariants. Capabilities' own hold in every state, as they do with one *)
(* lock; the ones after them are about the locks.                          *)

EmptySlotsAreBlank == C!EmptySlotsAreBlank
TreeWellFormed == C!TreeWellFormed
TreeIsDerivation == C!TreeIsDerivation
RightsNeverWiden == C!RightsNeverWiden
SameOrInnerObject == C!SameOrInnerObject
ParentCovers == C!ParentCovers
OriginsAcyclic == C!OriginsAcyclic
CapsImmutable == C!CapsImmutable
NoDerivationUnderRevoke == C!NoDerivationUnderRevoke
RevokeIsComplete == C!RevokeIsComplete

\* A clear flag means no relatives, and an empty slot's flag is clear.
LinkedIsSound ==
    \A s \in Slots :
        /\ s \notin C!Full => ~linked[s]
        /\ ~linked[s] => Unlinked(s)

\* A thread about to act on the fast path holds its slot's space, and the
\* slot is in no tree: it read the flag clear with the space locked, and
\* nobody could link the slot since.
FastPathAlone ==
    \A t \in Threads :
        (pc[t] = "commit" /\ ~op[t].slow /\ op[t].kind \in {"delete", "move"}) =>
            /\ held[SpaceOf(op[t].a)] = t
            /\ Unlinked(op[t].a)

\* The leaf a revoke is about to delete is still a leaf below the
\* capability, and its space is locked: nobody could change it since the
\* walk found it, as it is linked and the tree lock is held.
LeafStays ==
    \A t \in Threads :
        pc[t] = "leaf" =>
            /\ held[SpaceOf(op[t].b)] = t
            /\ op[t].b \in C!Descendants(op[t].a)
            /\ C!Descendants(op[t].b) = {}

\* Nobody waits for the tree lock holding a space's lock, space locks are
\* taken lowest first, and a revoke holds one at a time: so no two threads
\* wait for each other.
LockOrder ==
    \A t \in Threads :
        /\ pc[t] = "tree" => HeldBy(t) = {}
        /\ pc[t] = "spaces" =>
              \A x \in HeldBy(t), y \in Needed(op[t]) \ HeldBy(t) : x < y
        /\ pc[t] \in {"revoke", "down"} => tree = t /\ HeldBy(t) = {}
        /\ pc[t] \in {"mark", "again", "leaf"} => tree = t /\ Cardinality(HeldBy(t)) = 1

\* Every record counts exactly the capabilities naming its object, and the
\* operations under way that hold a count of it: a create's, made before
\* its capability is placed, and the count a fast path gives back after
\* its lock. So a count never falls below the capabilities naming its
\* object, and once nothing is under way it is exact.
Holds(t, o) ==
    \/ op[t].fin = o /\ pc[t] \in {"release", "finish"}
    \/ op[t].kind = "create" /\ op[t].o = o /\ pc[t] \in {"spaces", "commit"}
CountsAccounted ==
    \A o \in Objects :
        count[o] = Named(o) + Cardinality({t \in Threads : Holds(t, o)})

\* A free record's object is named by no capability, so the create that
\* takes the record next makes an object nobody can reach yet.
FreeRecordsNameNothing ==
    \A o \in Objects : count[o] = 0 => o \notin C!Live

\* A step on the fast path changes nothing outside its own slots: no link,
\* no other slot, in any space.
FastPathTouchesOnlyItsSlots ==
    [][\A t \in Threads :
          (pc[t] = "commit" /\ pc'[t] = "release" /\ ~op[t].slow) =>
              \A x \in Slots \ {op[t].a, op[t].b} :
                  /\ cap'[x] = cap[x] /\ parent'[x] = parent[x]
                  /\ depth'[x] = depth[x] /\ revoking'[x] = revoking[x]
                  /\ src'[x] = src[x] /\ linked'[x] = linked[x]]_vars

=============================================================================
