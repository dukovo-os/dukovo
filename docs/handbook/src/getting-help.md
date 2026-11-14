<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Getting help from someone

When something on a Dukovo computer will not work and someone else
could fix it, `help-session` lets them in without giving them anything
to keep. The owner starts a help session for one helper and a time; the
helper signs in over SSH with a code and sees and types in the owner's
own screen; the owner watches all of it and ends it with one key. The
helper needs no account on the computer and no software but `ssh`, and
nothing leaves your network or passes through anyone's servers. The full
reference is help-session(1); sshd(8) and authd(8) describe the other
side ([ADR 0540](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0540-a-helper-signs-in-to-the-owner-s-screen-over-ssh-and-rescue-takes-its-administrators-over-ssh.md)).

## What you give, and what you keep

A help session gives a stranger your rights for as long as it lasts, so
it is built to leave you in control:

- It never starts unless you start it, at your own terminal, and it
  ends when you press **Ctrl-]**. It also ends at its deadline, when you
  sign out, and when the helper leaves.
- The helper works in your shell, as you, in the same screen you see.
  Everything they type appears on your screen as they type it.
- Nothing in the shared screen ever asks for a password, so the helper
  cannot draw a fake question and read what you type. `elevate` there
  works only while you allow it, from another terminal.
- Every line the helper types is kept, with its time, in a file the
  helper cannot read or erase, and the audit log has each step.
- When it ends, everything the helper started in the screen ends too,
  including anything they left running in the background.

What the helper changed in your files and settings stays, as your own
changes would: the end of the session does not undo it. Give a code only
to someone you trust to work as you for the hour.

## Starting a session, as the owner

Run `help-session start` in a terminal, as yourself, not with `elevate`:

```
$ help-session start
Help session for alice until 15:04 (1 hour). Give your helper:
  ssh help@192.168.1.20
  the code KQ7R-M2XW-9HTP-4BZE
  this computer's key, which their ssh asks them to check:
    SHA256:Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7dE9f
They see and type in this screen, as you. Ctrl-] ends it at any time.
Everything they type is kept: help-session log shows it.
Waiting for your helper...
```

Tell the helper the address, the code and the key, by a way you trust,
such as a call. The code is made for this session and works once: the
first right sign-in spends it, and so do five wrong ones. `--for 90m`
(or `2h`, or a number of minutes) sets how long it lasts, from 5 minutes
to 8 hours, an hour without it. Only one session at a time is yours.

If the helper has an SSH key, give `--key` with their public key, as
`help-session start --key-file helper.pub`; then only that key signs in
and there is no code.

Once the helper signs in, the terminal shows the shell, and lines marked
`[help-session]` say what happens to the session: that the helper signed
in and from where, that elevate was allowed, and, 5 minutes and 1 minute
before the end, how much time is left. You can type in the screen too.

### Ending it

- Press **Ctrl-]** in the screen, at any time. It ends at once.
- From another terminal of yours, `help-session end`.
- Do nothing: it ends at its deadline.

The end says why and names the transcript. What your helper typed is
kept; `help-session log` lists your sessions and `help-session log ID`
shows one:

```
$ help-session log k7q2mx
```

An administrator sees everyone's with `elevate help-session log`.

### When the helper must run commands as an administrator

If you are an administrator and the helper needs `elevate`, allow it from
another terminal, which is not the shared screen:

```
$ help-session allow-elevate
Password:
elevate runs in your help screen for 15 minutes, without a password.
```

Without that, `elevate` in the screen says it runs only while you allow
it. The helper cannot ask for the allowance from inside the screen.

### When SSH is off

The helper reaches the computer over SSH, which is off until it is turned
on. An administrator can open it for the session alone:

```
$ help-session start --open-ssh
```

It asks for your password, starts `sshd` so that it takes the helper's
sign-in and no other, and stops it when the session ends. Without
`--open-ssh`, a session cannot start while SSH is off, and says so.

### Reaching the computer

The helper's computer must reach yours on port 22: on the same network, or
over a VPN, or through a port the router forwards to this computer. A
computer behind a home router, reached from anywhere, is not what
`help-session` does yet. The address in the message is this computer's
own address on each network it is on; give the one the helper can reach.
The firewall lets the helper's connection in, as it does any SSH
connection ([Networking](networking.md)).

## Helping someone, as the helper

You need `ssh` and what the owner gave you, and nothing of Dukovo's:

```
$ ssh help@192.168.1.20
The authenticity of host '192.168.1.20 (192.168.1.20)' can't be established.
ED25519 key fingerprint is SHA256:Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7dE9f.
Are you sure you want to continue connecting (yes/no/[fingerprint])? yes
help@192.168.1.20's password:
```

The first time, `ssh` shows the computer's key and asks whether to trust
it. Compare it with the key the owner read to you, and answer `yes` only
if they are the same: that tells you the machine is the owner's, and not
another that answered for it. The password prompt takes the code, typed
with or without its dashes and in any case. With a key, `ssh` signs in
with it and asks for nothing.

Then you are in the owner's screen:

- You see what the owner sees, from the moment you sign in. The screen
  starts blank; press **Ctrl-L** to redraw it, in a shell or an editor.
- You type in the owner's shell, as the owner, and the owner sees every
  key. They can type too.
- You have no terminal of your own, and no commands outside this screen,
  no `sftp` or `scp`, and no port forwarding. The size of your window
  does not change the screen's.
- A password is never asked in this screen. Say so to the owner if a
  task needs one: they run it from their own terminal.
- `elevate` works only if the owner, an administrator, allowed it, for 15
  minutes. If it says it runs only while the owner allows it, ask them
  for `help-session allow-elevate`.
- **Ctrl-]** from you is a key like any other. Only the owner's ends the
  session.
- What you type is kept for the owner, line by line, with its time.

If your connection is slower than the screen writes, what it cannot take
is skipped rather than slowing the owner, and you are told to press
Ctrl-L. When the session ends, for any reason, the screen says so and
`ssh` closes. A code works once, so a new session needs a new code.

Guessing a code is blocked like guessing a password: five wrong ones spend
the code, and many wrong tries from one address block the address for a
while ([SSH and remote access](ssh.md#blocking-addresses-that-guess-passwords)).

## What is recorded

Each step is a line of the audit log, which an administrator reads with
`log -s authd`: the start, the helper's sign-in with the address and
whether a code or a key, wrong codes, the allowing of elevate, and the
end with its reason. The transcript is in `/system/state/help/`, root's
alone: the 20 newest sessions of each owner are kept. It holds what the
helper typed, not what the programs showed.

## Limits

- A help session needs SSH to reach the computer: the same network, a
  VPN, or a forwarded port. A relay for computers behind a home router
  is not built.
- A computer in Rescue starts no help session. For a computer that will
  not start, see [Rescue](updates.md#rescue), which can be reached over
  SSH when the system had SSH on.
- A session belongs to a person: not to root, not to a service, and not
  to someone inside a help screen.
