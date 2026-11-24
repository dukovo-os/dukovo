\* SPDX-FileCopyrightText: 2026 The Dukovo contributors
\* SPDX-License-Identifier: Apache-2.0 OR MIT

------------------------------ MODULE ClockPage ------------------------------
(***************************************************************************)
(* A model of a page that the kernel writes and programs read with no      *)
(* lock, as a sequence lock: a counter, `seq`, and a few fields that make  *)
(* up one snapshot, such as the clock's base time and its scale.           *)
(*                                                                         *)
(* A writer takes the writer lock, which keeps the writers apart, makes    *)
(* `seq` odd, writes the fields one at a time, makes `seq` even again (one *)
(* more), and lets the lock go. Each writer writes a snapshot with a       *)
(* number from a global counter, so the fields of one snapshot all hold    *)
(* that number, and the fields of a page caught in the middle of a write   *)
(* differ. The page starts as snapshot 0, whole.                           *)
(*                                                                         *)
(* A reader reads `seq`, and starts again if it is odd, since a writer is  *)
(* in the middle of its work; reads the fields one at a time; reads `seq`  *)
(* again; and returns what it read only if `seq` has not changed, and      *)
(* starts again if it has. It takes no lock and writes nothing, so it      *)
(* costs a program no system call.                                         *)
(*                                                                         *)
(* Every read and write of `seq` and of a field is one step, in the order  *)
(* sequential consistency gives them; the code that the model stands for   *)
(* puts the fences between them that give the same order on the processors *)
(* the kernel runs on.                                                     *)
(*                                                                         *)
(* WholeSnapshot says a reading the reader returns has all its fields      *)
(* equal: it is one snapshot, never a mix of two. NotStale says that       *)
(* snapshot is not older than the last one fully written before the        *)
(* reader began its call. WholeSnapshot fails if the reader does not       *)
(* read `seq` the second time (`IF TRUE` in place of `IF first = seq` in   *)
(* `Check`): TLC finds a reader that reads `seq` = 0, a writer that        *)
(* makes it odd and writes the first field, and the reader that reads      *)
(* all three fields and returns snapshot 1's first field with snapshot     *)
(* 0's others. It fails too if the reader does not start again on an odd   *)
(* `seq` (`"fields"` in place of the test in `First`), since a reader      *)
(* that begins in the middle of a write reads a mix, and `seq` has not     *)
(* moved when it checks.                                                   *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    Writers,    \* threads that write the page
    Rounds,     \* how many snapshots each writer writes
    Fields      \* how many fields a snapshot has

ASSUME Rounds \in Nat /\ Fields \in Nat \ {0}

VARIABLES
    seq,        \* the sequence counter: odd while a writer is writing
    page,       \* the fields, each holding the number of the snapshot it is from
    owner,      \* who holds the writer lock
    counter,    \* the number of the last snapshot a writer began
    last,       \* the number of the last snapshot fully written
    wpc,        \* where each writer is
    mine,       \* the number of the snapshot each writer is writing
    wrote,      \* how many snapshots each writer has written
    at,         \* the next field a writer writes
    rpc,        \* where the reader is
    first,      \* the `seq` the reader read first
    next,       \* the next field the reader reads
    got,        \* the fields the reader has read
    floor,      \* the last snapshot fully written when the reader began
    ret,        \* what the reader returned
    back        \* the reader has returned

vars == <<seq, page, owner, counter, last, wpc, mine, wrote, at, rpc, first, next, got, floor,
          ret, back>>

None == "none"
Index == 1..Fields

WriterSteps == {"lock", "odd", "fields", "even", "unlock", "done"}
ReaderSteps == {"begin", "first", "fields", "check", "done"}

TypeOK ==
    /\ seq \in Nat
    /\ page \in [Index -> Nat]
    /\ owner \in Writers \cup {None}
    /\ counter \in Nat
    /\ last \in Nat
    /\ wpc \in [Writers -> WriterSteps]
    /\ mine \in [Writers -> Nat]
    /\ wrote \in [Writers -> 0..Rounds]
    /\ at \in [Writers -> 1..(Fields + 1)]
    /\ rpc \in ReaderSteps
    /\ first \in Nat
    /\ next \in 1..(Fields + 1)
    /\ got \in [Index -> Nat]
    /\ floor \in Nat
    /\ ret \in [Index -> Nat]
    /\ back \in BOOLEAN

Init ==
    /\ seq = 0
    /\ page = [i \in Index |-> 0]
    /\ owner = None
    /\ counter = 0
    /\ last = 0
    /\ wpc = [w \in Writers |-> IF Rounds > 0 THEN "lock" ELSE "done"]
    /\ mine = [w \in Writers |-> 0]
    /\ wrote = [w \in Writers |-> 0]
    /\ at = [w \in Writers |-> 1]
    /\ rpc = "begin"
    /\ first = 0
    /\ next = 1
    /\ got = [i \in Index |-> 0]
    /\ floor = 0
    /\ ret = [i \in Index |-> 0]
    /\ back = FALSE

----------------------------------------------------------------------------
(* A writer.                                                               *)

Lock(w) ==
    /\ wpc[w] = "lock" /\ owner = None
    /\ owner' = w
    /\ wpc' = [wpc EXCEPT ![w] = "odd"]
    /\ UNCHANGED <<seq, page, counter, last, mine, wrote, at, rpc, first, next, got, floor, ret, back>>

\* `seq` goes odd, and the writer takes the next snapshot number.
Odd(w) ==
    /\ wpc[w] = "odd"
    /\ seq' = seq + 1
    /\ counter' = counter + 1
    /\ mine' = [mine EXCEPT ![w] = counter + 1]
    /\ at' = [at EXCEPT ![w] = 1]
    /\ wpc' = [wpc EXCEPT ![w] = "fields"]
    /\ UNCHANGED <<page, owner, last, wrote, rpc, first, next, got, floor, ret, back>>

\* One field.
Field(w) ==
    /\ wpc[w] = "fields"
    /\ page' = [page EXCEPT ![at[w]] = mine[w]]
    /\ at' = [at EXCEPT ![w] = at[w] + 1]
    /\ wpc' = [wpc EXCEPT ![w] = IF at[w] = Fields THEN "even" ELSE "fields"]
    /\ UNCHANGED <<seq, owner, counter, last, mine, wrote, rpc, first, next, got, floor, ret, back>>

\* `seq` goes even again: the snapshot is whole.
Even(w) ==
    /\ wpc[w] = "even"
    /\ seq' = seq + 1
    /\ last' = mine[w]
    /\ wpc' = [wpc EXCEPT ![w] = "unlock"]
    /\ UNCHANGED <<page, owner, counter, mine, wrote, at, rpc, first, next, got, floor, ret, back>>

Unlock(w) ==
    /\ wpc[w] = "unlock"
    /\ owner' = None
    /\ wrote' = [wrote EXCEPT ![w] = wrote[w] + 1]
    /\ wpc' = [wpc EXCEPT ![w] = IF wrote[w] + 1 < Rounds THEN "lock" ELSE "done"]
    /\ UNCHANGED <<seq, page, counter, last, mine, at, rpc, first, next, got, floor, ret, back>>

Writer(w) == Lock(w) \/ Odd(w) \/ Field(w) \/ Even(w) \/ Unlock(w)

----------------------------------------------------------------------------
(* The reader.                                                             *)

\* The call begins: what is whole by now is the least it may return.
Begin ==
    /\ rpc = "begin"
    /\ floor' = last
    /\ rpc' = "first"
    /\ UNCHANGED <<seq, page, owner, counter, last, wpc, mine, wrote, at, first, next, got, ret, back>>

\* The first read of `seq`: odd means a writer is at work, and the reader
\* starts again.
First ==
    /\ rpc = "first"
    /\ first' = seq
    /\ next' = 1
    /\ rpc' = IF seq % 2 = 1 THEN "first" ELSE "fields"
    /\ UNCHANGED <<seq, page, owner, counter, last, wpc, mine, wrote, at, got, floor, ret, back>>

\* One field.
Read ==
    /\ rpc = "fields"
    /\ got' = [got EXCEPT ![next] = page[next]]
    /\ next' = next + 1
    /\ rpc' = IF next = Fields THEN "check" ELSE "fields"
    /\ UNCHANGED <<seq, page, owner, counter, last, wpc, mine, wrote, at, first, floor, ret, back>>

\* The second read of `seq`: unchanged, and what was read is a snapshot;
\* changed, and the reader starts again.
Check ==
    /\ rpc = "check"
    /\ IF first = seq
       THEN /\ ret' = got
            /\ back' = TRUE
            /\ rpc' = "done"
       ELSE /\ rpc' = "first"
            /\ UNCHANGED <<ret, back>>
    /\ UNCHANGED <<seq, page, owner, counter, last, wpc, mine, wrote, at, first, next, got, floor>>

Reader == Begin \/ First \/ Read \/ Check

----------------------------------------------------------------------------

Done == rpc = "done" /\ \A w \in Writers : wpc[w] = "done"

Next ==
    \/ \E w \in Writers : Writer(w)
    \/ Reader
    \/ Done /\ UNCHANGED vars

Spec ==
    /\ Init /\ [][Next]_vars
    /\ \A w \in Writers : WF_vars(Writer(w))
    /\ WF_vars(Reader)

\* A reading the reader returned is one snapshot: all its fields equal.
WholeSnapshot ==
    back => \A i, j \in Index : ret[i] = ret[j]

\* And one not older than the last fully written when the call began.
NotStale ==
    back => ret[1] >= floor

\* The reader returns in the end.
ReaderReturns == <>(rpc = "done")

=============================================================================
