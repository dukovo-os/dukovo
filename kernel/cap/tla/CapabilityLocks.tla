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
(* model derives with every right and no badge. The object table is       *)
(* guarded by a lock of its own, taken last and only around a step, so it  *)
(* is part of the steps here.                                              *)
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
    op          \* what each thread is doing

store == <<cap, parent, depth, revoking, origin, src>>
vars == <<cap, parent, depth, revoking, origin, src, linked, tree, held, pc, op>>

C == INSTANCE Capabilities

Spaces == {1, 2}
SpaceOf(s) == IF s \in Low THEN 1 ELSE 2

\* A slot in no tree: no parent, no children (so no siblings either).
Unlinked(s) == parent[s] = None /\ \A x \in Slots : parent[x] # s

\* What a thread does. `a` is the slot it works on, `b` the other one
\* (where a move or a derive puts the capability, the leaf a revoke
\* deletes), `o` the object a create makes, `slow` says whether it is on
\* the slow path, and `begun` whether a revoke has set its mark.
Kinds == {"create", "delete", "move", "derive", "revoke"}
Ops == [kind : Kinds, a : Slots, b : Slots, o : Objects, slow : BOOLEAN, begun : BOOLEAN]

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
            slow |-> FALSE, begun |-> FALSE]

HeldBy(t) == {x \in Spaces : held[x] = t}

Least(set) == CHOOSE x \in set : \A y \in set : x <= y

Steps == {"idle", "tree", "spaces", "check", "commit", "release",
          "revoke", "mark", "again", "down", "leaf"}

TypeOK ==
    /\ C!TypeOK
    /\ linked \in [Slots -> BOOLEAN]
    /\ tree \in Threads \cup {None}
    /\ held \in [Spaces -> Threads \cup {None}]
    /\ pc \in [Threads -> Steps]
    /\ op \in [Threads -> Ops]

Init ==
    /\ C!Init
    /\ linked = [s \in Slots |-> FALSE]
    /\ tree = None
    /\ held = [x \in Spaces |-> None]
    /\ pc = [t \in Threads |-> "idle"]
    /\ op = [t \in Threads |-> Nothing]

-----------------------------------------------------------------------------
(* Taking and letting go of locks.                                         *)

\* A thread picks what to do next, from what it sees of the store with no
\* lock held (which may change before it acts). Creating, deleting and
\* moving try the fast path; deriving and revoking go straight to the slow
\* one.
Start(t) ==
    /\ pc[t] = "idle"
    /\ \E k \in Kinds, a \in Slots, b \in Slots, o \in Objects :
          /\ (k = "create") = (a \notin C!Full)
          /\ k \in {"move", "derive"} => a # b /\ b \notin C!Full
          /\ k \notin {"move", "derive"} => b = a
          /\ k = "create" => o \notin C!Live
          /\ k # "create" => o = CHOOSE x \in Objects : TRUE
          /\ op' = [op EXCEPT ![t] = [kind |-> k, a |-> a, b |-> b, o |-> o,
                                      slow |-> k \in {"derive", "revoke"},
                                      begun |-> FALSE]]
          /\ pc' = [pc EXCEPT ![t] = IF k \in {"derive", "revoke"} THEN "tree" ELSE "spaces"]
    /\ UNCHANGED <<store, linked, tree, held>>

\* The tree lock, holding no space's lock (`LockOrder`).
TakeTree(t) ==
    /\ pc[t] = "tree"
    /\ tree = None
    /\ tree' = t
    /\ pc' = [pc EXCEPT ![t] = IF op[t].kind = "revoke" THEN "revoke" ELSE "spaces"]
    /\ UNCHANGED <<store, linked, held, op>>

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
    /\ UNCHANGED <<store, linked, tree, op>>

\* Every lock the thread holds goes at once (the order of letting go
\* does not matter).
Release(t) ==
    /\ pc[t] = "release"
    /\ tree' = IF tree = t THEN None ELSE tree
    /\ held' = [x \in Spaces |-> IF held[x] = t THEN None ELSE held[x]]
    /\ pc' = [pc EXCEPT ![t] = "idle"]
    /\ op' = [op EXCEPT ![t] = Nothing]
    /\ UNCHANGED <<store, linked>>

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
    /\ UNCHANGED <<store, linked, tree>>

\* The new object's original capability, in an empty slot, with no
\* relatives: the store's create, which links nothing.
FastCreate(o) ==
    IF o.a \notin C!Full /\ o.o \notin C!Live
    THEN /\ cap' = [cap EXCEPT ![o.a] = [obj |-> o.o, rights |-> Rights, badge |-> 0]]
         /\ origin' = [origin EXCEPT ![o.o] = None]
         /\ UNCHANGED <<parent, depth, revoking, src, linked>>
    ELSE UNCHANGED <<store, linked>>

\* Empties the slot and touches nothing else: no link is followed or
\* changed.
FastDelete(o) ==
    /\ cap' = [cap EXCEPT ![o.a] = NoCap]
    /\ depth' = [depth EXCEPT ![o.a] = 0]
    /\ revoking' = [revoking EXCEPT ![o.a] = FALSE]
    /\ src' = [src EXCEPT ![o.a] = {}]
    /\ UNCHANGED <<parent, origin, linked>>

\* Carries the capability and its depth to the other slot, which may be
\* in the other space, and touches nothing else.
FastMove(o) ==
    /\ cap' = [cap EXCEPT ![o.b] = cap[o.a], ![o.a] = NoCap]
    /\ depth' = [depth EXCEPT ![o.b] = depth[o.a], ![o.a] = 0]
    /\ src' = [src EXCEPT ![o.b] = src[o.a], ![o.a] = {}]
    /\ UNCHANGED <<parent, revoking, origin, linked>>

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

SlowStep(o) ==
    CASE o.kind = "delete" ->
            IF o.a \in C!Full THEN C!Remove(o.a) /\ ClearParent(o.a)
            ELSE UNCHANGED <<store, linked>>
      [] o.kind = "move" ->
            IF o.a \in C!Full /\ o.b \notin C!Full /\ ~revoking[o.a]
            THEN /\ C!Move(o.a, o.b)
                 /\ linked' = [linked EXCEPT ![o.b] = linked[o.a], ![o.a] = FALSE]
            ELSE UNCHANGED <<store, linked>>
      [] o.kind = "derive" ->
            IF DeriveOK(o.a, o.b)
            THEN /\ C!Derive(o.a, o.b, Rights, 0)
                 /\ linked' = [linked EXCEPT ![o.a] = TRUE, ![o.b] = TRUE]
            ELSE UNCHANGED <<store, linked>>

\* The step itself, with every lock it needs held.
Commit(t) ==
    /\ pc[t] = "commit"
    /\ LET o == op[t]
       IN  CASE o.kind = "create" -> FastCreate(o)
             [] o.slow -> SlowStep(o)
             [] o.kind = "delete" -> FastDelete(o)
             [] o.kind = "move" -> FastMove(o)
    /\ pc' = [pc EXCEPT ![t] = "release"]
    /\ UNCHANGED <<tree, held, op>>

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
    /\ UNCHANGED <<store, linked, tree, op>>

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
    /\ UNCHANGED linked

\* Each later turn looks at the mark again, as `continue_revoke` does.
\* Without it (another revoke of the capability finished, or the
\* capability was deleted, perhaps with another put in its slot), what
\* was asked is done. With nothing left below the capability, the mark is
\* cleared. Otherwise the turn lets go of the capability's space and goes
\* down to a leaf.
Again(t) ==
    /\ pc[t] = "again"
    /\ LET s == op[t].a
       IN  CASE ~revoking[s] ->
                    /\ pc' = [pc EXCEPT ![t] = "release"]
                    /\ UNCHANGED <<store, held>>
             [] C!Descendants(s) = {} ->
                    /\ C!RevokeDone(s)
                    /\ pc' = [pc EXCEPT ![t] = "release"]
                    /\ UNCHANGED held
             [] OTHER ->
                    /\ held' = [x \in Spaces |-> IF held[x] = t THEN None ELSE held[x]]
                    /\ pc' = [pc EXCEPT ![t] = "down"]
                    /\ UNCHANGED store
    /\ UNCHANGED <<linked, tree, op>>

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
    /\ UNCHANGED <<store, linked, tree>>

Leaf(t) ==
    /\ pc[t] = "leaf"
    /\ C!Remove(op[t].b)
    /\ ClearParent(op[t].b)
    /\ NextTurn(t)
    /\ UNCHANGED op

-----------------------------------------------------------------------------

Thread(t) ==
    \/ Start(t) \/ TakeTree(t) \/ TakeSpaces(t) \/ Check(t) \/ Commit(t)
    \/ Release(t) \/ Revoke(t) \/ Mark(t) \/ Again(t) \/ Down(t) \/ Leaf(t)

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
