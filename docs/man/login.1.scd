login(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

login - sign in at a console

# SYNOPSIS

*login*

*login --session* [*--command* _command_]

# DESCRIPTION

*login* signs people in at Dukovo's consoles (ADR 0250). *consoled*(8)
runs it as root on each console's terminal, after the console's
_/system/settings/issue_ text. It asks for a name, then for a password,
unseen, and sends both, with the terminal, to *authd*(8), which checks
the password and starts the person's session on that terminal: their
shell, acting for their user and groups, in their home. login then
waits, holding nothing of the terminal, until the session ends, and
ends too; consoled clears the console and starts it again.

A wrong name or password is answered, after a wait that grows with
each wrong one in a row, with *The name or the password is wrong.*,
whichever it was, and the name is asked again. The right password of a
disabled account (*user*(8)) is answered with *The account* _name_ *is
disabled*. Every sign-in and sign-out, and every wrong password, is a
security event in the audit log (*authd*(8)), and authd records each
for *last*(1), *lastb*(1) and *lastlog*(8) (ADR 0254). Before the shell
starts, authd says when the person last signed in, and the wrong
passwords typed for them since:

```
Last sign-in: Sat Oct 31 12:40 2026 on pts/0
Since then, 1 wrong password was typed for bob, the last on Sat Oct 31 12:44 2026 on pts/1.
```

Each console signs in on its own: two people, or one person twice, can
be signed in on two consoles at once, and *who*(1) lists them.

# THE FIRST SIGN-IN

On a machine with no account yet, made without *ob image --user*,
login first makes the owner's account, an administrator's (*user*(8)):
a name, a full name, which may be empty, and a password, twice. No
rules are forced on the password: an empty one, or one shorter than 8
characters, gets a warning and the question whether to keep it. Then
it signs the owner in. Only the first account is made this way; the
owner makes the others with *user add*.

```
Dukovo Server 0.15.0 (tty1)

Welcome to Dukovo. This computer has no account yet: make yours, the owner's,
which administers it. Nothing but a name and a password is needed, and the
password may be empty.

Name: alice
Full name (may be empty): Alice Liddell
New password:
The same again:
Made alice, number 1000, home /users/alice

$
```

# STARTUP TROUBLE

For an administrator's lost password, the boot menu's second entry,
which Shift held as the computer starts shows (ADR 0426),
*Dukovo Server* _version_*: startup trouble*, starts the computer with
the kernel flag *startup-trouble* (*dukovo-boot*(7)), as Ubuntu's
recovery mode does. login then shows its menu before each sign-in
(ADR 0253):

```
Startup trouble: this computer was started from its boot menu's
startup-trouble entry.

  1  Reset an administrator's password
  2  Sign in
  3  Restart
  4  Power off

Choose 1 to 4: 1

Administrators: alice
Whose password? alice
New password:
The same again:
The password of alice is reset. Every administrator is told at their
next sign-in, and the audit log keeps it.
```

The new password is asked twice and warned about as at the first
sign-in, never refused. *authd*(8) resets it only while the computer
runs for startup trouble, and tells each administrator at their next
sign-in, before their shell starts:

```
Your password was reset from the startup-trouble menu on Sat Oct 31 12:40 2026.
```

A disabled administrator's account is listed as *(disabled)*, and
resetting its password enables it too (ADR 0220): it is how the owner
gets back in.

Anyone at the boot menu could already start the computer with root's
shell (*console-sign-in=off*), so the menu gives no one more; what it
adds is that the owner is never locked out of their own computer, and
that nobody resets a password unseen. Restart and Power off ask
*spark*(8), as *reboot*(8) and *poweroff*(8) do. Once the disks are
encrypted (release 1.4), resetting a password will need the recovery
key.

# A FAILED START

While the notice of a start that failed its health check is shown
(*sysupdate*(8), ADR 0501), which *consoled*(8) and *serial-console*(8) show
above the sign-in until the next start, login offers three keys after it
(ADR 0532) and reads one, without Enter:

```
Press R to report this failure, Q for its support page as a QR code, or Enter to sign in.
```

*Q*
	Shows the QR code of the support page that the notice names, with the
	address under it, and *Press a key to go back.* A phone's camera reads
	the code. A terminal too narrow or too low for the code gets the
	address alone. The choices are shown again after a key. When the
	notice names no support page, *Q* is not a choice.

*R*
	Says *Sign in as an administrator to report it.*, asks for the name
	and the password, and signs the person in with the command *report
	failure* (*report*(1)) in place of their shell: the session is the
	report, and ends with it. Only an administrator may make that report;
	anyone else is told so by *report*. A wrong name or password shows the
	choices again.

Enter
	Signs in as usual: the name is asked.

Any other printable key
	Signs in as usual, with the key as the first letter of the name. A name
	that starts with *R* or *Q* is typed after Enter.

Other keys, such as Space and the arrow keys, are ignored. A sign-in at a
console with no failure notice asks for the name at once. On a computer
that has no account yet, the owner's account is made first and the keys
are not offered.

# THE SESSION

authd runs *login --session* as the session's first process, acting
for the person who signed in, with the terminal as its standard input,
output and error. It starts a new session (*setsid*(2)) with the
terminal as its controlling terminal, goes to *HOME*, or to _/_ with a
message if it cannot, and runs *SHELL*, *nukesh*(1) unless the account
names another, as a login shell: its name starts with *-*. The
environment authd gives it is *HOME*, *SHELL*, *USER* and *LOGNAME* from
the account, *PATH=/system/commands*, *TERM* from the console,
*LANG=C.UTF-8* and *EDITOR=nano*.

A session that signed in over SSH (*sshd*(8), ADR 0347) runs the same
way, on the terminal sshd made for it, or on three pipes when the client
asked for none; then there is no controlling terminal to take. With
*--command*, it runs the command the client asked for with the shell's
*-c*, the shell named without the *-*, so that it reads no login files,
as Linux's sshd runs one.

# OPTIONS

*--session*
	Run as a session's first process, as above. authd passes it.

*--command* _command_
	In a session, run _command_ with *SHELL -c* instead of a login
	shell. authd passes it for an SSH client's command, and for the
	command that the failure notice's *R* signs in with.

# EXIT STATUS

0 when a session started and ended; 1 when the input ended first, or
authd could not be reached in 30 seconds, saying so.

# SEE ALSO

*authd*(8), *consoled*(8), *report*(1), *serial-console*(8), *sshd*(8), *sysupdate*(8), *user*(8), *passwd*(1), *who*(1), *last*(1),
*lastlog*(8), *nukesh*(1), *dukovo-boot*(7), *systools*(1)
