\* SPDX-FileCopyrightText: 2026 The Dukovo contributors
\* SPDX-License-Identifier: Apache-2.0 OR MIT

----------------------------- MODULE IdleWake ------------------------------
(***************************************************************************)
(* A model of how an idle CPU waits, how other CPUs wake it, and how they  *)
(* hand it the threads they wake for it (kernel/radix/src/sched/mod.rs:    *)
(* `idle_loop`, `poll`, `kick`, `hand_over`, `take_handed` and            *)
(* `reschedule`, and `radix_sched::Resched`, ADR 0555 and ADR 0556).       *)
(*                                                                         *)
(* The CPU's request to switch is one word, alone in its cache line: a     *)
(* `pending` bit, which wakers set, a `watched` bit, which the idle CPU    *)
(* sets while it polls the word or waits in MWAIT on its line, and the     *)
(* CPU's inbox, the threads other CPUs woke for it and have yet to be     *)
(* queued. A waker either queues the thread on the CPU's run queue itself *)
(* and then sets `pending`, or hands it over: puts it in the inbox and     *)
(* sets `pending` in one step. Either way the step that sets `pending`     *)
(* also reads `watched`, and if the word was not watched the waker then    *)
(* sends an interrupt.                                                     *)
(*                                                                         *)
(* The idle CPU, with interrupts masked, checks `pending` and its queue    *)
(* and then either polls (as long as its recent idle periods suggest, ADR  *)
(* 0037 and ADR 0551) or goes straight to its sleep. Polling sets          *)
(* `watched`, spins with interrupts on until `pending` is set or its time  *)
(* is up, then clears `watched` and reads `pending` in one atomic step.    *)
(* Sleeping with MWAIT sets `watched` and reads `pending` in one step,     *)
(* arms the monitor and waits; sleeping with HLT reads `pending` and       *)
(* halts (STI; HLT, so an interrupt that is already pending ends the halt  *)
(* at once).                                                               *)
(*                                                                         *)
(* Every switch begins by taking the word: clearing it, `pending` and the  *)
(* inbox at once, and queuing the inbox's threads. A thread about to       *)
(* block may find it was woken since it decided to, and carry on; if the   *)
(* word held a request, the CPU then chooses again, since the request it   *)
(* took may have been for a thread that should run now. A CPU that runs a  *)
(* thread is interrupted by a waker that hands it one, and switches on the *)
(* way out of the interrupt; it may also notice `pending` sooner, as it    *)
(* lets go of a spin lock.                                                 *)
(*                                                                         *)
(* Each step of the model is one access to the word, one atomic            *)
(* read-modify-write of it, or one critical section under the run queue's *)
(* lock, as in the code, so the interleavings of the model's steps cover   *)
(* every execution. Whether the CPU has MWAIT and whether it polls are     *)
(* chosen at the start, so one run checks all four combinations, and a     *)
(* poll may end at any moment, which covers every polling limit.           *)
(*                                                                         *)
(* NoLostWakeup says a CPU never sleeps with `pending` set and nothing on  *)
(* its way to wake it; TakenUnwatched that no switch takes the word while  *)
(* the idle thread watches it; RequestHonoured that a switch that took a  *)
(* request chooses again before the CPU runs on. EveryThreadRuns says      *)
(* every thread woken for the CPU, queued or handed over, ends up running. *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    Wakers,     \* CPUs that wake a thread for this one, each its own thread
    None        \* no thread: the CPU runs its idle thread

\* The thread each waker wakes, named after it.
Threads == Wakers

VARIABLES
    mwait,      \* the CPU waits with MONITOR and MWAIT (else HLT)
    pollFirst,  \* it polls before sleeping (every CPU may, ADR 0551)
    pending,    \* the word's pending bit: a switch is asked for
    watched,    \* the word's watched bit
    inbox,      \* the word's inbox: threads handed over, not yet queued
    irq,        \* an interrupt is pending for it
    queue,      \* threads on its run queue, the running one apart
    cur,        \* the thread it runs, or None
    ran,        \* threads that ran here and then blocked for good
    early,      \* a thread may yet find itself woken as it blocks
    owed,       \* the switch under way took a request, not yet acted on
    ipc,        \* where the CPU is
    wpc,        \* where each waker is
    unwatched   \* each waker found the word unwatched, so must interrupt

vars == <<mwait, pollFirst, pending, watched, inbox, irq, queue, cur, ran, early, owed,
          ipc, wpc, unwatched>>

CpuSteps ==
    {"top", "poll", "unpoll", "prepare", "asleep", "woken", "switch", "busy", "block",
     "blocking"}
WakerSteps == {"wake", "ring", "signal", "done"}

TypeOK ==
    /\ mwait \in BOOLEAN
    /\ pollFirst \in BOOLEAN
    /\ pending \in BOOLEAN
    /\ watched \in BOOLEAN
    /\ inbox \subseteq Threads
    /\ irq \in BOOLEAN
    /\ queue \subseteq Threads
    /\ cur \in Threads \cup {None}
    /\ ran \subseteq Threads
    /\ early \in BOOLEAN
    /\ owed \in BOOLEAN
    /\ ipc \in CpuSteps
    /\ wpc \in [Wakers -> WakerSteps]
    /\ unwatched \in [Wakers -> BOOLEAN]

Init ==
    /\ mwait \in BOOLEAN
    /\ pollFirst \in BOOLEAN
    /\ pending = FALSE
    /\ watched = FALSE
    /\ inbox = {}
    /\ irq = FALSE
    /\ queue = {}
    /\ cur = None
    /\ ran = {}
    /\ early = TRUE
    /\ owed = FALSE
    /\ ipc = "top"
    /\ wpc = [w \in Wakers |-> "wake"]
    /\ unwatched = [w \in Wakers |-> FALSE]

\* Choosing under the run queue's lock among `ready`: the CPU runs one of
\* them, or its idle thread if there are none.
Pick(ready) ==
    IF ready = {}
    THEN /\ cur' = None /\ queue' = {} /\ ipc' = "top"
    ELSE \E t \in ready : cur' = t /\ queue' = ready \ {t} /\ ipc' = "busy"

----------------------------------------------------------------------------
(* The idle CPU (idle_loop and poll).                                      *)

\* With interrupts masked, `go_idle` and the queue's count: switch if a
\* thread waits, else start waiting.
Top ==
    /\ ipc = "top"
    /\ ipc' = IF pending \/ queue # {} THEN "switch"
              ELSE IF pollFirst THEN "poll" ELSE "prepare"
    /\ UNCHANGED <<mwait, pollFirst, pending, watched, inbox, irq, queue, cur, ran, early,
                   owed, wpc, unwatched>>

\* Polling sets the watched bit, then spins with interrupts on: an
\* interrupt is taken and handled, and the poll ends when `pending` is seen
\* or at any moment (its time is up).
StartPoll ==
    /\ ipc = "poll" /\ ~watched
    /\ watched' = TRUE
    /\ UNCHANGED <<mwait, pollFirst, pending, inbox, irq, queue, cur, ran, early, owed, ipc,
                   wpc, unwatched>>

TakeInterruptWhilePolling ==
    /\ ipc = "poll" /\ watched /\ irq
    /\ irq' = FALSE
    /\ UNCHANGED <<mwait, pollFirst, pending, watched, inbox, queue, cur, ran, early, owed,
                   ipc, wpc, unwatched>>

EndPoll ==
    /\ ipc = "poll" /\ watched
    /\ ipc' = "unpoll"
    /\ UNCHANGED <<mwait, pollFirst, pending, watched, inbox, irq, queue, cur, ran, early,
                   owed, wpc, unwatched>>

\* Interrupts masked again, `unwatch`: clears the watched bit and reads
\* `pending` in one step.
Unpoll ==
    /\ ipc = "unpoll"
    /\ watched' = FALSE
    /\ ipc' = IF pending THEN "switch" ELSE "prepare"
    /\ UNCHANGED <<mwait, pollFirst, pending, inbox, irq, queue, cur, ran, early, owed, wpc,
                   unwatched>>

\* Going to sleep. MWAIT watches the word's line, so it sets the watched
\* bit and reads `pending` in one step (`watch`); HLT only reads `pending`,
\* and a waker that sees the word unwatched interrupts it.
Prepare ==
    /\ ipc = "prepare"
    /\ watched' = mwait
    /\ ipc' = IF pending THEN "woken" ELSE "asleep"
    /\ UNCHANGED <<mwait, pollFirst, pending, inbox, irq, queue, cur, ran, early, owed, wpc,
                   unwatched>>

\* STI; HLT or MWAIT: a pending interrupt ends the sleep (and is handled),
\* and so does a waker's write to the watched line under MWAIT. A write
\* before the monitor is armed is caught by the check that follows it, so
\* the model lets MWAIT end whenever `pending` is set.
WakeUp ==
    /\ ipc = "asleep"
    /\ irq \/ (mwait /\ pending)
    /\ irq' = FALSE
    /\ ipc' = "woken"
    /\ UNCHANGED <<mwait, pollFirst, pending, watched, inbox, queue, cur, ran, early, owed,
                   wpc, unwatched>>

\* `unwatch` after MWAIT; HLT never set the bit.
Woken ==
    /\ ipc = "woken"
    /\ watched' = FALSE
    /\ ipc' = "top"
    /\ UNCHANGED <<mwait, pollFirst, pending, inbox, irq, queue, cur, ran, early, owed, wpc,
                   unwatched>>

----------------------------------------------------------------------------
(* Switching (reschedule and take_handed).                                 *)

\* A switch that is not a block: takes the word, queues the inbox's
\* threads, and chooses among everything ready, the running thread too.
Switch ==
    /\ ipc = "switch"
    /\ pending' = FALSE
    /\ inbox' = {}
    /\ Pick(queue \cup inbox \cup (IF cur = None THEN {} ELSE {cur}))
    /\ UNCHANGED <<mwait, pollFirst, watched, irq, ran, early, owed, wpc, unwatched>>

\* The running thread, between switches: it may decide to block, take an
\* interrupt, and switch on the way out if `pending` is set, or notice
\* `pending` as it lets go of a spin lock.
Run ==
    /\ ipc = "busy"
    /\ ipc' = "block"
    /\ UNCHANGED <<mwait, pollFirst, pending, watched, inbox, irq, queue, cur, ran, early,
                   owed, wpc, unwatched>>

TakeInterruptWhileBusy ==
    /\ ipc = "busy" /\ irq
    /\ irq' = FALSE
    /\ ipc' = IF pending THEN "switch" ELSE "busy"
    /\ UNCHANGED <<mwait, pollFirst, pending, watched, inbox, queue, cur, ran, early, owed,
                   wpc, unwatched>>

NoticePending ==
    /\ ipc = "busy" /\ pending
    /\ ipc' = "switch"
    /\ UNCHANGED <<mwait, pollFirst, pending, watched, inbox, irq, queue, cur, ran, early,
                   owed, wpc, unwatched>>

\* reschedule(Block) begins: takes the word and queues the inbox's
\* threads, before the thread's own lock.
BeginBlock ==
    /\ ipc = "block"
    /\ owed' = pending
    /\ pending' = FALSE
    /\ inbox' = {}
    /\ queue' = queue \cup inbox
    /\ ipc' = "blocking"
    /\ UNCHANGED <<mwait, pollFirst, watched, irq, cur, ran, early, wpc, unwatched>>

\* Under its lock, the thread sleeps and the CPU chooses another ...
Sleep ==
    /\ ipc = "blocking"
    /\ ran' = ran \cup {cur}
    /\ owed' = FALSE
    /\ Pick(queue)
    /\ UNCHANGED <<mwait, pollFirst, pending, watched, inbox, irq, early, wpc, unwatched>>

\* ... or finds it was woken since it decided to block. It carries on,
\* unless the word held a request: then the CPU chooses again, as if the
\* thread were preempted.
CarryOn ==
    /\ ipc = "blocking" /\ early
    /\ early' = FALSE
    /\ IF owed
       THEN owed' = FALSE /\ Pick(queue \cup {cur})
       ELSE ipc' = "busy" /\ UNCHANGED <<queue, cur, owed>>
    /\ UNCHANGED <<mwait, pollFirst, pending, watched, inbox, irq, ran, wpc, unwatched>>

\* What the CPU does that it must keep doing: noticing `pending` while it
\* lets go of a spin lock may happen, but need not.
Cpu ==
    \/ Top \/ StartPoll \/ TakeInterruptWhilePolling \/ EndPoll \/ Unpoll
    \/ Prepare \/ WakeUp \/ Woken
    \/ Switch \/ Run \/ TakeInterruptWhileBusy \/ BeginBlock \/ Sleep \/ CarryOn

----------------------------------------------------------------------------
(* The wakers (wake_masked, enqueue_on with kick, and hand_over).          *)

\* Queues the thread on the CPU's run queue, under its lock ...
Enqueue(w) ==
    /\ wpc[w] = "wake"
    /\ queue' = queue \cup {w}
    /\ wpc' = [wpc EXCEPT ![w] = "ring"]
    /\ UNCHANGED <<mwait, pollFirst, pending, watched, inbox, irq, cur, ran, early, owed,
                   ipc, unwatched>>

\* ... then `request_remote`: sets `pending` and reads the watched bit in
\* one step.
Ring(w) ==
    /\ wpc[w] = "ring"
    /\ pending' = TRUE
    /\ unwatched' = [unwatched EXCEPT ![w] = ~watched]
    /\ wpc' = [wpc EXCEPT ![w] = "signal"]
    /\ UNCHANGED <<mwait, pollFirst, watched, inbox, irq, queue, cur, ran, early, owed, ipc>>

\* Or `hand_over`: puts the thread in the inbox, sets `pending` and reads
\* the watched bit, in one step.
HandOver(w) ==
    /\ wpc[w] = "wake"
    /\ inbox' = inbox \cup {w}
    /\ pending' = TRUE
    /\ unwatched' = [unwatched EXCEPT ![w] = ~watched]
    /\ wpc' = [wpc EXCEPT ![w] = "signal"]
    /\ UNCHANGED <<mwait, pollFirst, watched, irq, queue, cur, ran, early, owed, ipc>>

\* The interrupt, if the word was unwatched.
Signal(w) ==
    /\ wpc[w] = "signal"
    /\ irq' = (irq \/ unwatched[w])
    /\ wpc' = [wpc EXCEPT ![w] = "done"]
    /\ UNCHANGED <<mwait, pollFirst, pending, watched, inbox, queue, cur, ran, early, owed,
                   ipc, unwatched>>

Waker(w) == Enqueue(w) \/ Ring(w) \/ HandOver(w) \/ Signal(w)

----------------------------------------------------------------------------

\* Every thread has run, and the CPU sleeps with nothing left to do.
Done ==
    /\ \A w \in Wakers : wpc[w] = "done"
    /\ ran = Threads
    /\ ipc = "asleep"

Next ==
    \/ Cpu \/ NoticePending
    \/ \E w \in Wakers : Waker(w)
    \/ Done /\ UNCHANGED vars

Spec == Init /\ [][Next]_vars /\ WF_vars(Cpu) /\ \A w \in Wakers : WF_vars(Waker(w))

\* Asleep with `pending` set, the CPU has an interrupt pending, or waits
\* in MWAIT on the line the request was written to, or has a waker that
\* has yet to signal.
NoLostWakeup ==
    (ipc = "asleep" /\ pending) =>
        \/ irq
        \/ mwait /\ watched
        \/ \E w \in Wakers : wpc[w] = "signal"

\* The inbox is in the word, and a hand-over sets `pending` with it.
InboxPending == inbox # {} => pending

\* A switch never takes the word while the idle thread watches it.
TakenUnwatched == ipc \in {"switch", "block"} => ~watched

\* A request a switch took is acted on before the CPU runs on.
RequestHonoured == ipc = "busy" => ~owed

\* A thread is in one place at a time: the inbox, the queue, running or
\* done.
OnePlace ==
    /\ inbox \cap queue = {}
    /\ cur \notin inbox \cup queue \cup ran
    /\ ran \cap (inbox \cup queue) = {}

\* Once a waker has asked for the switch, the CPU switches.
EventuallyTaken == pending ~> ~pending

\* Every thread woken for the CPU runs.
EveryThreadRuns == <>(ran = Threads)

=============================================================================
