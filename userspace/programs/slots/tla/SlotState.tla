\* SPDX-FileCopyrightText: 2026 The Dukovo contributors
\* SPDX-License-Identifier: Apache-2.0 OR MIT

------------------------------ MODULE SlotState ------------------------------
(***************************************************************************)
(* A model of the slot state of Dukovo's atomic updates, with power cuts   *)
(* anywhere (ADR 0452; userspace/programs/slots, the GRUB menu in          *)
(* image/grub/installed.cfg.in and image/grub/menu.cfg, and sysupdate(8)). *)
(*                                                                         *)
(* The system runs from one of two slots, `a` and `b`. The state is kept  *)
(* in two copies on the EFI system partition, each written in place and   *)
(* each torn, no longer whole, by a power cut during its write; of two     *)
(* whole copies the newer counts, by its generation, 0, 1 and 2 round.     *)
(* At each start GRUB reads the copies, picks the slot, and writes the     *)
(* next state over the older copy before it starts the slot: a try taken  *)
(* from the update being tried, and the start marked as not seen through. *)
(* The system writes over the older copy too, or over one not whole: its  *)
(* health check, once the start has come up; a stage, which forgets what  *)
(* the spare slot held on both copies, removes it, builds the update in    *)
(* its place, and names it the update to try; a rollback; an unstage.     *)
(*                                                                         *)
(* A slot holds a version that proves itself (`ok`), one whose start never *)
(* comes up, as a kernel that panics or a Spark that hangs, and which a    *)
(* restart ends (`crash`), one that comes up and fails its health check    *)
(* (`sick`), or nothing while a stage rebuilds it.                         *)
(*                                                                         *)
(* GRUB may find it cannot write a copy, and then writes nothing: the      *)
(* default starts, so that no try is started without being counted.       *)
(*                                                                         *)
(* The checks, in every state: one copy is always whole; GRUB never        *)
(* starts an empty slot; an update that does not prove itself starts at   *)
(* most `Tries` times; the default and the previous version are versions   *)
(* that proved themselves; and every slot the state names holds a system.  *)
(* With `StartDefault` FALSE, GRUB writes over the newer copy when it      *)
(* cannot write the older, as a first draft of the menu did, and TLC finds *)
(* two cuts leaving neither copy whole; with `ForgetFirst` FALSE, a stage  *)
(* removes the spare slot while a copy still names it (configurations     *)
(* `ob test tla` does not run).                                            *)
(***************************************************************************)
EXTENDS Naturals, TLC

CONSTANTS
    Tries,       \* the tries an update gets
    MaxStages,   \* how many updates are staged
    MaxCuts,     \* how many times the power is cut
    StartDefault,\* whether GRUB, when it cannot write, starts the default
    ForgetFirst, \* whether a stage forgets the spare slot before removing it
    Torn,        \* a copy a power cut tore, not whole (a model value)
    None         \* nothing (a model value)

Slots == {"a", "b"}
Other(s) == IF s = "a" THEN "b" ELSE "a"
NextGen(g) == (g + 1) % 3
Bad == {"crash", "sick"}
Kinds == {"ok"} \cup Bad

States ==
    [gen : 0..2, default : Slots, previous : Slots \cup {None},
     trying : Slots \cup {None}, tries : 0..Tries, boot : Slots \cup {None}]

\* What an install writes into both copies.
Installed ==
    [gen |-> 0, default |-> "a", previous |-> None, trying |-> None, tries |-> 0,
     boot |-> None]

VARIABLES
    copy,     \* the two copies of the state
    content,  \* what each slot holds
    pc,       \* "grub", "kernel" or "up"
    running,  \* the slot started, or None
    write,    \* a write under way: the copy it goes over and the state, or None
    then,     \* what follows the write: "kernel", "up", "restart" or "forgot"
    checked,  \* whether this start's health check has run
    stage,    \* how far a stage has got: "idle", "forget", "remove", "build", "name"
    starts,   \* starts of each slot holding a bad update since a stage put it there
    stages,   \* stages begun
    cuts      \* power cuts

vars == <<copy, content, pc, running, write, then, checked, stage, starts, stages,
          cuts>>

Whole(i) == copy[i] # Torn
SomeWhole == Whole(1) \/ Whole(2)
BothWhole == Whole(1) /\ Whole(2)

\* The copy that counts: the newer whole one.
Newer ==
    IF BothWhole
    THEN IF copy[2].gen = NextGen(copy[1].gen) THEN 2 ELSE 1
    ELSE IF Whole(1) THEN 1 ELSE 2
Read == copy[Newer]

\* The copy the system writes over: the older whole one, or the one that is
\* not whole; it writes the newer's next generation.
SysCopy == IF BothWhole THEN 3 - Newer ELSE IF Whole(1) THEN 2 ELSE 1
SysState == IF SomeWhole THEN Read ELSE [Installed EXCEPT !.default = running]
SysGen == IF SomeWhole THEN NextGen(Read.gen) ELSE 0

\* Begins writing `st` over copy `i`: until it ends, the copy is not whole.
Begin(i, st, after) ==
    /\ copy' = [copy EXCEPT ![i] = Torn]
    /\ write' = [at |-> i, st |-> st]
    /\ then' = after

\* A system write of `st`, the next generation, followed by `after`.
SysWrite(st, after) == Begin(SysCopy, [st EXCEPT !.gen = SysGen], after)

-----------------------------------------------------------------------------
(* The state's transitions, as userspace/programs/slots has them.          *)

\* What the health check of a start of `r` writes, `passed` or not
\* (State::checked).
Checked(st, r, passed) ==
    IF st.trying = r /\ passed
    THEN [st EXCEPT !.default = r,
                    !.previous = IF st.default # r THEN st.default ELSE None,
                    !.trying = None, !.tries = 0, !.boot = None]
    ELSE IF st.trying = r
    THEN [st EXCEPT !.tries = 0]
    ELSE IF st.trying # None /\ st.tries = 0 /\ r = st.default
    THEN [st EXCEPT !.trying = None, !.boot = None]
    ELSE [st EXCEPT !.boot = None]

\* What a stage writes before it removes the spare slot (State::forget).
Forget(st, s) ==
    [st EXCEPT !.previous = IF st.previous = s THEN None ELSE st.previous,
               !.trying = IF st.trying = s THEN None ELSE st.trying,
               !.tries = IF st.trying = s THEN 0 ELSE st.tries]
Names(st, s) == st.previous = s \/ st.trying = s

-----------------------------------------------------------------------------
(* GRUB, at each start.                                                    *)

\* What GRUB goes by: the newer whole copy, with the other slot the
\* default if the default's menu cannot be read (it is being rebuilt).
GrubState ==
    IF content[Read.default] # "empty" THEN Read
    ELSE [Read EXCEPT !.default = Other(Read.default), !.previous = None,
                      !.trying = None, !.tries = 0]

\* The copy GRUB writes over: the older, or the one that is not whole.
GrubCopy == IF BothWhole THEN 3 - Newer ELSE IF Whole(1) THEN 2 ELSE 1

Grub ==
    /\ pc = "grub"
    /\ write = None
    /\ IF ~SomeWhole
       THEN \* Slot A, or B if A cannot be read; nothing written.
            /\ running' = IF content["a"] # "empty" THEN "a" ELSE "b"
            /\ pc' = "kernel"
            /\ UNCHANGED <<copy, write, then>>
       ELSE LET st == GrubState
                s == IF st.trying # None /\ st.tries > 0 THEN st.trying ELSE st.default
                next == [st EXCEPT !.gen = NextGen(Read.gen),
                                   !.tries = IF st.trying # None /\ st.tries > 0
                                             THEN st.tries - 1 ELSE st.tries,
                                   !.boot = s]
            IN \/ \* The next state over the older copy, then the slot.
                  /\ Begin(GrubCopy, next, "kernel")
                  /\ running' = s
                  /\ pc' = "grub"
               \/ \* GRUB cannot write the copy, and leaves it as it was:
                  \* the default starts, the update's tries waiting; or, as a
                  \* first draft of the menu did, it writes over the newer
                  \* copy, as the next generation.
                  /\ IF StartDefault
                     THEN /\ running' = st.default
                          /\ pc' = "kernel"
                          /\ UNCHANGED <<copy, write, then>>
                     ELSE /\ Begin(Newer, next, "kernel")
                          /\ running' = s
                          /\ pc' = "grub"
    /\ UNCHANGED <<content, checked, stage, starts, stages, cuts>>

\* A write ends.
Finish ==
    /\ write # None
    /\ copy' = [copy EXCEPT ![write.at] = write.st]
    /\ write' = None
    /\ then' = None
    /\ CASE then = "kernel" -> pc' = "kernel" /\ UNCHANGED running
         [] then = "restart" -> pc' = "grub" /\ running' = None
         [] OTHER -> UNCHANGED <<pc, running>>
    /\ stage' = CASE then = "forgot" /\ stage = "forget" -> "forget2"
                  [] then = "forgot" -> "remove"
                  [] then = "named" -> "idle"
                  [] OTHER -> stage
    /\ UNCHANGED <<content, checked, starts, stages, cuts>>

\* The kernel starts the slot: a bad update's start counts.
Kernel ==
    /\ pc = "kernel"
    /\ write = None
    /\ starts' = [starts EXCEPT ![running] =
                     IF content[running] \in Bad THEN IF @ > Tries THEN @ ELSE @ + 1
                     ELSE @]
    /\ checked' = FALSE
    /\ IF content[running] = "crash"
       THEN pc' = "grub" /\ running' = None
       ELSE pc' = "up" /\ UNCHANGED running
    /\ UNCHANGED <<copy, content, write, then, stage, stages, cuts>>

-----------------------------------------------------------------------------
(* The system, once a start has come up. The health check and the other    *)
(* writers take a lock, so they run one at a time.                         *)

Idle == pc = "up" /\ write = None /\ stage = "idle"

\* The health check, which restarts the machine when an update it tries
\* fails.
Health ==
    /\ Idle
    /\ ~checked
    /\ LET st == SysState
           passed == content[running] = "ok"
           next == Checked(st, running, passed)
           failed == st.trying = running /\ ~passed
       IN IF next # st
          THEN /\ SysWrite(next, IF failed THEN "restart" ELSE "up")
               /\ UNCHANGED <<pc, running>>
          ELSE /\ UNCHANGED <<copy, write, then>>
               /\ IF failed THEN pc' = "grub" /\ running' = None
                  ELSE UNCHANGED <<pc, running>>
    /\ checked' = TRUE
    /\ UNCHANGED <<content, stage, starts, stages, cuts>>

\* A stage begins, from the default.
StageBegin ==
    /\ Idle
    /\ stages < MaxStages
    /\ SomeWhole
    /\ running = Read.default
    /\ stage' = IF ForgetFirst /\ \E i \in 1..2 : Whole(i) /\ Names(copy[i], Other(running))
                 THEN "forget" ELSE "remove"
    /\ stages' = stages + 1
    /\ UNCHANGED <<copy, content, pc, running, write, then, checked, starts, cuts>>

\* It forgets the spare slot, over one copy and then the other.
StageForget ==
    /\ pc = "up" /\ write = None
    /\ stage \in {"forget", "forget2"}
    /\ SysWrite(Forget(SysState, Other(running)), "forgot")
    /\ UNCHANGED <<content, pc, running, checked, stage, starts, stages, cuts>>

\* It removes what the spare slot held, and builds the update there.
StageRemove ==
    /\ pc = "up" /\ write = None /\ stage = "remove"
    /\ content' = [content EXCEPT ![Other(running)] = "empty"]
    /\ stage' = "build"
    /\ UNCHANGED <<copy, pc, running, write, then, checked, starts, stages, cuts>>

StageBuild ==
    /\ pc = "up" /\ write = None /\ stage = "build"
    /\ \E k \in Kinds : content' = [content EXCEPT ![Other(running)] = k]
    /\ starts' = [starts EXCEPT ![Other(running)] = 0]
    /\ stage' = "name"
    /\ UNCHANGED <<copy, pc, running, write, then, checked, stages, cuts>>

\* It names the update to try (State::stage).
StageName ==
    /\ pc = "up" /\ write = None /\ stage = "name"
    /\ SysWrite([SysState EXCEPT !.previous = None, !.trying = Other(running),
                                 !.tries = Tries], "named")
    /\ UNCHANGED <<content, pc, running, checked, stage, starts, stages, cuts>>

\* sysupdate rollback, refused while an update is staged or tried.
Rollback ==
    /\ Idle
    /\ SomeWhole
    /\ Read.trying = None
    /\ Read.previous \notin {None, Read.default}
    /\ content[Read.previous] # "empty"
    /\ SysWrite([Read EXCEPT !.default = Read.previous, !.previous = Read.default], "up")
    /\ UNCHANGED <<content, pc, running, checked, stage, starts, stages, cuts>>

\* sysupdate unstage, of an update not running.
Unstage ==
    /\ Idle
    /\ SomeWhole
    /\ Read.trying \notin {None, running}
    /\ SysWrite([Read EXCEPT !.trying = None, !.tries = 0], "up")
    /\ UNCHANGED <<content, pc, running, checked, stage, starts, stages, cuts>>

\* A restart asked for, once the health check has run.
Restart ==
    /\ Idle
    /\ checked
    /\ pc' = "grub"
    /\ running' = None
    /\ UNCHANGED <<copy, content, write, then, checked, stage, starts, stages, cuts>>

\* The power is cut: a write under way leaves its copy torn, and a stage
\* under way stops where it is.
Cut ==
    /\ cuts < MaxCuts
    /\ pc' = "grub"
    /\ running' = None
    /\ write' = None
    /\ then' = None
    /\ stage' = "idle"
    /\ cuts' = cuts + 1
    /\ UNCHANGED <<copy, content, checked, starts, stages>>

Init ==
    /\ copy = [i \in 1..2 |-> Installed]
    /\ content = [s \in Slots |-> IF s = "a" THEN "ok" ELSE "empty"]
    /\ pc = "grub"
    /\ running = None
    /\ write = None
    /\ then = None
    /\ checked = FALSE
    /\ stage = "idle"
    /\ starts = [s \in Slots |-> 0]
    /\ stages = 0
    /\ cuts = 0

Next ==
    \/ Grub \/ Finish \/ Kernel
    \/ Health \/ StageBegin \/ StageForget \/ StageRemove \/ StageBuild \/ StageName
    \/ Rollback \/ Unstage \/ Restart \/ Cut

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* The checks.                                                             *)

TypeOK ==
    /\ copy \in [1..2 -> States \cup {Torn}]
    /\ content \in [Slots -> Kinds \cup {"empty"}]
    /\ pc \in {"grub", "kernel", "up"}
    /\ running \in Slots \cup {None}
    /\ stage \in {"idle", "forget", "forget2", "remove", "build", "name"}
    /\ starts \in [Slots -> 0..Tries + 1]

\* One copy is always whole.
OneWhole == SomeWhole

\* GRUB starts only a slot that holds a system.
StartsASystem == pc = "kernel" => content[running] # "empty"

\* An update that does not prove itself starts at most `Tries` times.
TriesCounted == \A s \in Slots : starts[s] <= Tries

\* The default and the previous version proved themselves, and the
\* update being tried is there.
NamesHeld ==
    SomeWhole =>
        /\ content[Read.default] = "ok"
        /\ Read.previous # None => content[Read.previous] = "ok"
        /\ Read.trying # None => content[Read.trying] # "empty"
=============================================================================
