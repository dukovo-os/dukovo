\* SPDX-FileCopyrightText: 2026 The Dukovo contributors
\* SPDX-License-Identifier: Apache-2.0 OR MIT

----------------------------- MODULE IdleWake ------------------------------
(***************************************************************************)
(* A model of how an idle CPU waits and how other CPUs wake it            *)
(* (kernel/radix/src/sched/mod.rs: `idle_loop`, `poll` and `kick`, and     *)
(* `radix_sched::Resched`, ADR 0555).                                      *)
(*                                                                         *)
(* The CPU's request to switch is one word, alone in its cache line: a     *)
(* `pending` bit, which wakers set, and a `watched` bit, which the idle    *)
(* CPU sets while it polls the word or waits in MWAIT on its line. A       *)
(* waker sets `pending` and reads `watched` in one atomic read-modify-     *)
(* write; if the word was not watched it then sends an interrupt. The idle *)
(* CPU, with interrupts masked, checks `pending` and then either polls (as *)
(* long as its recent idle periods suggest, ADR 0037 and ADR 0551) or goes *)
(* straight to its sleep. Polling sets `watched`, spins with interrupts on *)
(* until `pending` is set or its time is up, then clears `watched` and     *)
(* reads `pending` in one atomic step. Sleeping with MWAIT sets `watched`  *)
(* and reads `pending` in one step, arms the monitor and waits; sleeping   *)
(* with HLT reads `pending` and halts (STI; HLT, so an interrupt that is   *)
(* already pending ends the halt at once).                                *)
(*                                                                         *)
(* Each step of the model is one access to the word, or one atomic        *)
(* read-modify-write of it, as in the code, so the interleavings of the    *)
(* model's steps cover every execution. Whether the CPU has MWAIT and      *)
(* whether it polls are chosen at the start, so one run checks all four    *)
(* combinations, and a poll may end at any moment, which covers every      *)
(* polling limit.                                                          *)
(*                                                                         *)
(* NoLostWakeup says a CPU never sleeps with `pending` set and nothing on  *)
(* its way to wake it; EventuallyRuns says it always ends up switching to  *)
(* the woken thread. Because each side sets its own bit and reads the     *)
(* other's in one atomic step, no ordering between separate accesses is   *)
(* left for the code to get wrong.                                        *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    Wakers      \* CPUs that wake the idle one

VARIABLES
    mwait,      \* the CPU waits with MONITOR and MWAIT (else HLT)
    pollFirst,  \* it polls before sleeping (every CPU may, ADR 0551)
    pending,    \* the word's pending bit: a switch is asked for
    watched,    \* the word's watched bit
    irq,        \* an interrupt is pending for it
    ipc,        \* where the idle CPU is in its loop
    wpc,        \* where each waker is in `kick`
    unwatched   \* each waker found the word unwatched, so must interrupt

vars == <<mwait, pollFirst, pending, watched, irq, ipc, wpc, unwatched>>

IdleSteps == {"top", "poll", "unpoll", "prepare", "asleep", "woken", "run"}
WakerSteps == {"ring", "signal", "done"}

TypeOK ==
    /\ mwait \in BOOLEAN
    /\ pollFirst \in BOOLEAN
    /\ pending \in BOOLEAN
    /\ watched \in BOOLEAN
    /\ irq \in BOOLEAN
    /\ ipc \in IdleSteps
    /\ wpc \in [Wakers -> WakerSteps]
    /\ unwatched \in [Wakers -> BOOLEAN]

Init ==
    /\ mwait \in BOOLEAN
    /\ pollFirst \in BOOLEAN
    /\ pending = FALSE
    /\ watched = FALSE
    /\ irq = FALSE
    /\ ipc = "top"
    /\ wpc = [w \in Wakers |-> "ring"]
    /\ unwatched = [w \in Wakers |-> FALSE]

----------------------------------------------------------------------------
(* The idle CPU (idle_loop and poll).                                      *)

\* With interrupts masked, `go_idle`: switch if a thread is waiting, else
\* start waiting.
Top ==
    /\ ipc = "top"
    /\ ipc' = IF pending THEN "run" ELSE IF pollFirst THEN "poll" ELSE "prepare"
    /\ UNCHANGED <<mwait, pollFirst, pending, watched, irq, wpc, unwatched>>

\* Polling sets the watched bit, then spins with interrupts on: an
\* interrupt is taken and handled, and the poll ends when `pending` is seen
\* or at any moment (its time is up).
StartPoll ==
    /\ ipc = "poll" /\ ~watched
    /\ watched' = TRUE
    /\ UNCHANGED <<mwait, pollFirst, pending, irq, ipc, wpc, unwatched>>

TakeInterruptWhilePolling ==
    /\ ipc = "poll" /\ watched /\ irq
    /\ irq' = FALSE
    /\ UNCHANGED <<mwait, pollFirst, pending, watched, ipc, wpc, unwatched>>

EndPoll ==
    /\ ipc = "poll" /\ watched
    /\ ipc' = "unpoll"
    /\ UNCHANGED <<mwait, pollFirst, pending, watched, irq, wpc, unwatched>>

\* Interrupts masked again, `unwatch`: clears the watched bit and reads
\* `pending` in one step.
Unpoll ==
    /\ ipc = "unpoll"
    /\ watched' = FALSE
    /\ ipc' = IF pending THEN "run" ELSE "prepare"
    /\ UNCHANGED <<mwait, pollFirst, pending, irq, wpc, unwatched>>

\* Going to sleep. MWAIT watches the word's line, so it sets the watched
\* bit and reads `pending` in one step (`watch`); HLT only reads `pending`,
\* and a waker that sees the word unwatched interrupts it.
Prepare ==
    /\ ipc = "prepare"
    /\ watched' = mwait
    /\ ipc' = IF pending THEN "woken" ELSE "asleep"
    /\ UNCHANGED <<mwait, pollFirst, pending, irq, wpc, unwatched>>

\* STI; HLT or MWAIT: a pending interrupt ends the sleep (and is handled),
\* and so does a waker's write to the watched line under MWAIT. A write
\* before the monitor is armed is caught by the check that follows it, so
\* the model lets MWAIT end whenever `pending` is set.
WakeUp ==
    /\ ipc = "asleep"
    /\ irq \/ (mwait /\ pending)
    /\ irq' = FALSE
    /\ ipc' = "woken"
    /\ UNCHANGED <<mwait, pollFirst, pending, watched, wpc, unwatched>>

\* `unwatch` after MWAIT; HLT never set the bit.
Woken ==
    /\ ipc = "woken"
    /\ watched' = FALSE
    /\ ipc' = "top"
    /\ UNCHANGED <<mwait, pollFirst, pending, irq, wpc, unwatched>>

Idle ==
    \/ Top \/ StartPoll \/ TakeInterruptWhilePolling \/ EndPoll \/ Unpoll
    \/ Prepare \/ WakeUp \/ Woken

----------------------------------------------------------------------------
(* The wakers (kick).                                                      *)

\* `request_remote`: sets `pending` and reads the watched bit in one step.
Ring(w) ==
    /\ wpc[w] = "ring"
    /\ pending' = TRUE
    /\ unwatched' = [unwatched EXCEPT ![w] = ~watched]
    /\ wpc' = [wpc EXCEPT ![w] = "signal"]
    /\ UNCHANGED <<mwait, pollFirst, watched, irq, ipc>>

\* The interrupt, if the word was unwatched.
Signal(w) ==
    /\ wpc[w] = "signal"
    /\ irq' = (irq \/ unwatched[w])
    /\ wpc' = [wpc EXCEPT ![w] = "done"]
    /\ UNCHANGED <<mwait, pollFirst, pending, watched, ipc, unwatched>>

Waker(w) == Ring(w) \/ Signal(w)

----------------------------------------------------------------------------

Done == ipc = "run" /\ \A w \in Wakers : wpc[w] = "done"

Next ==
    \/ Idle
    \/ \E w \in Wakers : Waker(w)
    \/ Done /\ UNCHANGED vars

Spec == Init /\ [][Next]_vars /\ WF_vars(Idle) /\ \A w \in Wakers : WF_vars(Waker(w))

\* Asleep with `pending` set, the CPU has an interrupt pending, or waits
\* in MWAIT on the line the request was written to, or has a waker that
\* has yet to signal.
NoLostWakeup ==
    (ipc = "asleep" /\ pending) =>
        \/ irq
        \/ mwait /\ watched
        \/ \E w \in Wakers : wpc[w] # "done"

\* Once a waker has asked for the switch, the idle CPU switches.
EventuallyRuns == pending ~> (ipc = "run")

=============================================================================
