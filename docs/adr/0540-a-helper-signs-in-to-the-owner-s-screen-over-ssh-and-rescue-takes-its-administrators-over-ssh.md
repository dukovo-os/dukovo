<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0540. A helper signs in to the owner's screen over SSH, and Rescue takes its administrators over SSH

- Status: Accepted
- Date: 2026-11-14

## Context

Two kinds of remote help were missing, and both put a stranger or an
administrator in front of a machine's most trusted parts.

A person who is stuck has no safe way to let someone help. Windows has
Quick Assist and TeamViewer, which go through a company's servers and
accounts; on Linux people share a password or a tmux socket, which gives
the helper everything and leaves the owner no record. The troubleshooting
design asks for a session the owner starts for one helper, for a time, that
the owner watches and ends with one key, and that needs no account.

A computer that will not start can be repaired only at its screen. Rescue
(ADRs 0459, 0460 and 0501) listens on nothing, so a headless server that
fails to start cannot be repaired from where its owner reached it before.
`docs/deferred.md` listed SSH into Rescue when the system had SSH on, to
wait for the outside security review, which both parts of this record go
to.

authd already starts every session and checks every SSH sign-in (ADRs 0347
and 0348), and Spark gives a service the capabilities its file asks for.

## Decision

### Help sessions

1. **The owner starts it, for one helper, for a time.** `help-session
   start [--for DURATION] [--key KEY | --key-file FILE] [--open-ssh]`,
   run by a person, never by root, a service or from inside a help screen,
   in their terminal. It lasts 5 minutes to 8 hours, an hour by default,
   and a person has one at a time. It prints an address for each address
   of the computer other machines reach, a code and sshd's host key
   fingerprint, which the helper's `ssh` asks them to check.
2. **The helper needs no account.** They sign in over SSH as `help`, a name
   no account may take. The code is 16 characters from a 32-letter
   alphabet without look-alikes (80 bits), typed with or without dashes in
   any case, and is spent by the first right sign-in or by five wrong ones.
   With `--key`, only that public key signs in and no code is made. authd
   checks the code or the key's signature; sshd offers passwords for `help`,
   and for no other name, only while a session waits for a code
   (`/system/live/authd/help-waiting`), even when sshd_config turns
   passwords off. `help` passes AllowUsers and AllowGroups.
3. **The helper shares the owner's screen.** help-session makes a
   terminal pair, the help screen, shows it in the owner's terminal, and
   hands authd the terminal and one end of a socket pair. authd starts the
   owner's login shell on the terminal, as the owner, with `HELP_SESSION`
   set; when the helper signs in, authd gives sshd the socket and a
   transcript, and sshd passes the helper's channel to and from the
   socket. The helper gets one terminal and one shell: no command, no SFTP
   and no forwarding, and the screen keeps the owner's size whatever the
   helper's window is. The owner may type too.
4. **The helper's screen starts at their sign-in and never holds the
   owner's back.** It shows what changes from then on, and Ctrl-L redraws
   it. The output waits for a slow connection up to 256 KiB; past that,
   what is waiting is skipped and the helper is told to redraw. sshd tells
   the helper when the session ends, since authd closes the socket as it
   ends it.
5. **The session ends in one key, or when its time is up.** Ctrl-] in the
   owner's terminal, `help-session end` from another of theirs, the
   deadline (both screens are told 5 minutes and 1 minute before), the end
   of the help shell or of the helper's connection, and authd's or sshd's
   restart all end it. authd starts the shell in a resource domain of its
   own below its own (radix-resource-domain(2)) and stops the domain, as
   Spark stops a service, so that every process the helper started,
   including any left in the background, ends. authd holds a capability to
   its own domain through a new permission, `domain = true`, which only
   its service file sets. What the helper changed in the owner's files and
   settings stays, as the owner's own changes would; the end says so and
   names the transcript.
6. **Everything the helper types is kept where the helper cannot reach
   it.** sshd, which runs as root, writes each line with its time to
   `/system/state/help/ID.log`, mode 0600, before it passes it on, and ends
   the connection at 1 MiB so that nothing goes unkept. `help-session log`
   shows the owner's own and, under elevate, everyone's; the 20 newest of
   each owner are kept. authd's audit lines name each step: the start, the
   helper's sign-in with its address and code or key, the wrong codes,
   elevate allowed, and the end with its reason.
7. **Nothing asks for a password in the help screen,** since the helper
   could draw the question and read the answer. authd tells a process of
   the screen by its place in the session's domain and refuses there
   `unlock`, `passwd`, a sharing password, the keyring's requests and
   `help-start`, saying to run it from another terminal. `elevate` in the
   screen is refused with "In a help session, elevate runs only while its
   owner allows it: help-session allow-elevate." An administrator owner
   runs `help-session allow-elevate` in another terminal, where authd asks
   their password, and elevate in the screen then runs for 15 minutes with
   no password and in the session's domain, so that the end stops it too.
8. **Each wrong code counts for the address** as a wrong password does
   (ADR 0479), so a guesser is blocked, and a block's refusal ends a
   connection open before it.
9. **SSH must reach the machine.** When sshd is off, an administrator may
   run `help-session start --open-ssh`: authd asks their password, writes
   `/system/live/authd/help-only`, which makes sshd take `help` sign-ins
   only, has Spark start sshd, and stops it again at the end. A computer
   behind a home router is reached from the same network, a VPN or a
   forwarded port; a relay is in `docs/deferred.md`.

### SSH into Rescue

10. **SSH comes up in Rescue only if the system had it on.** Before it asks
    at the screen, dukovo-rescue checks that the root's sshd service is
    enabled (its file among the root's settings, or the default slot's,
    with the override over it), that the root's accounts read and name an
    administrator, and that the root's host key is there. If so it turns
    SSH on; if not, nothing listens. Either way it prints one line: that
    SSH is on, as it was on this computer, or that networking is off and
    SSH stays off, with the reason. Turning networking on by hand never
    starts sshd. A second console finds it running and starts nothing.
11. **Rescue links the root where only root may write.** dukovo-rescue
    links the root it mounted at `/system/live/rescue/root`, in a folder
    root made with mode 0700, and not at `/tmp/rescue/root`: a link in a
    folder anyone writes could name accounts of someone else's making.
    Only a system the boot menu started with `rescue=` on the kernel's
    command line reads the link, and only one that root made.
12. **authd reads the root through the link; sshd never sees it.** authd
    holds the whole tree and takes the accounts, password hashes and keys
    from the root. sshd sees only its own folders, so dukovo-rescue copies
    it the five files it reads, sshd_config, passwd, group, the host key
    and its public half, into `/system/state/sshd/rescue`, mode 0600, in
    Rescue's memory. The program that faces the network never sees the
    password hashes or a home. sshd makes no host key in Rescue, so a
    client sees the fingerprint it knows and no warning.
13. **Rescue takes the root's network settings,** the cards' addresses,
    the DNS, the firewall's rules and blocking.toml, where the root has
    them, so that the machine is where its owner reached it before; the
    firewall loads them before networking starts. Blocks are kept in
    Rescue's memory.
14. **In Rescue, authd answers only SSH's sign-ins and the blocked list,
    and only for the root's administrators, root aside, and writes nothing
    to the root.** Every other request, a change of an account, a help
    session, elevate, is refused with "Rescue changes no account". The
    root's AllowUsers, AllowGroups, DenyUsers and DenyGroups apply, and
    passwords sign in only if the root's sshd_config lets them. A root
    whose accounts cannot be read gives no SSH, though the screen asks for
    no password then.
15. **An administrator's session is Rescue's menu.** authd starts
    dukovo-rescue as root with `--ssh NAME`, on the terminal sshd opened, as
    an elevated command runs and with no password asked again; the menu says
    who signed in and from where, and its "Open a shell" is the shell. A
    client that asks for a command, SFTP or forwarding is refused. `--ssh`
    is authd's: another caller is refused.
16. **One repair at a time.** The choices that change something, reinstall,
    putting back settings, the disk check, copying files off, starting
    normally, restart and power off, take a lock, flock(2) on
    `/system/live/rescue/lock`, which the kernel lets go however its
    holder ends. Its line names who, what and from where, so that a second
    person's choice says "alice is reinstalling Dukovo, over SSH from
    192.168.1.5: wait for it to finish, then choose again". Choices that
    only read run side by side.
17. **The boot tests `help-session` and `rescue-ssh`, and the lab tests
    `security/help-session` and `updates/rescue-ssh`, cover them.**
    help-session(1), authd(8), sshd(8), dukovo-rescue(8) and elevate(8)
    describe them, and the handbook has "Getting help from someone".

## What it is not

- No SFTP in Rescue, to copy files off over the network: the menu's own copy
  to another disk covers it (`docs/deferred.md`).
- No help session across a home router, which needs a relay
  (`docs/deferred.md`), and none in Rescue.
- The helper gets no terminal of their own: they share the owner's.
- A help session does not undo what the helper changed.

## Alternatives

- **A service run by a company,** as Quick Assist and TeamViewer are, needs
  an account and sends the screen away. Here nothing leaves the network
  and the helper needs only `ssh`.
- **Sharing a password or a tmux socket** gives the helper the owner's
  account for good and keeps no record. A code is spent once, ends with
  the session, and every line the helper types is kept out of their reach.
- **A separate terminal for the helper** would leave the owner unable to
  watch it; one shared screen is watched by construction.
- **Letting elevate ask in the help screen** would let the helper fake the
  prompt; allowing it from outside the screen cannot be faked from inside.
- **Reading the root from sshd** would put the password hashes and the
  homes in the program that faces the network; the five copies hold what
  sshd reads and nothing more.
- **A link in `/tmp/rescue`** is writable by anyone; the folder only root
  writes is not.
- **A lock file that records its holder** goes stale when the holder dies;
  flock(2) lets go with the descriptor.

## Consequences

- A person gets help from someone with no account, for as long as they
  choose, watching all of it, and ends it with Ctrl-].
- A helper can do in the screen what the owner can, except what asks for a
  password; they can read and change the owner's files for the session.
  The transcript shows what they typed, not what the programs showed.
- Both parts go to the outside security review, which reads authd's help
  and Rescue code, sshd's `help` sign-in and the copies.
- A headless server that will not start can be repaired over SSH by the
  administrators and with the keys it had before, from where its owner
  reached it, and the host key the clients know.
- Rescue's SSH adds networking, authd and sshd to a system meant to stay
  small, and only when the system had SSH on.
- authd's service file carries one permission no other service holds.
