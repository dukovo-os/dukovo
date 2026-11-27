authd(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

authd - keep the accounts and check passwords

# SYNOPSIS

*authd*

# DESCRIPTION

authd keeps Dukovo's accounts and their passwords (ADR 0218). Spark runs
it as the *authd* service. It alone changes the account files, and it
alone reads the passwords' hashes, it alone starts the sessions of
people who sign in, and it alone runs commands as an administrator:
other programs ask it over a socket, as *login*(1), *user*(8),
*passwd*(1) and *elevate*(8) do.

An account is a name its owner chooses and a password, which may be
empty: nothing else, and never an age, a birth date or an identity
(ADR 0126). Each lives in three files:

- _/system/settings/passwd_ and _/system/settings/group_, in Linux's
  formats, which every program reads (*getpwnam*(3), *ls -l*, *id*(1))
  and which hold no password;
- _/system/state/authd/passwords_, a line _name_*:*_hash_*:* for each
  account, as Linux's _/etc/shadow_ begins, root's and of mode 0600 in a
  folder of mode 0700, which no other service's view holds
  (*service.toml*(5)).

A hash is Argon2id (RFC 9106), at the RFC's second recommendation: 64
MiB of memory, 3 passes and 4 lanes, with a salt of 16 random bytes,
written as a PHC string, as libxcrypt and Python's argon2-cffi write it:

```
alice:$argon2id$v=19$m=65536,t=3,p=4$c29tZXNhbHQ...$GpZ3sK/oH9p7...:
```

A hash says its cost, so one moved from Linux, or made at another cost,
is checked all the same, up to 1 GiB, 16 passes and 16 lanes. An empty
hash is an empty password; an account with no line, or a line authd
cannot read, is locked: no password opens it.

A person may also have a sharing password, which Windows PCs sign in to
the folders *smbd*(8) shares with (ADR 0228), set with *share
password* (*share*(1)). NTLMv2, which the PCs sign in with, needs the
password's NT hash, the MD4 of its UTF-16, which is as good as the
password to anyone who reads it; so it is never the login password's,
and is kept apart, in _/system/state/authd/sharing_, a line
_name_*:*_hash_*:* each, the hash in hexadecimal, root's and of mode
0600 beside the passwords. An account without a line signs in to no
share.
authd writes each file anew beside the old one, syncs it and renames it
over the old one, the hashes first and the passwd file last, so that an
account is there once its user is. Lines it does not read, an
administrator's comments among them, are kept as they are.

No rules are forced on a password (ADR 0218): *user* and *passwd* warn
about an empty one, or one shorter than 8 characters, and ask whether to
keep it; the person who answers decides.

# THE SOCKET

authd answers at _/system/live/authd/socket_, which anyone may connect
to. A request is its fields, each ending in a NUL byte, after which the
client shuts down its end for writing; authd answers with one line,
*ok* or *ok* _text_ when it did what was asked, or *no* _why_, then
closes the connection, but for a session (below):

*check* _name_ _password_
	Whether _password_ is _name_'s. Root asks this of any account;
	another program only of the account it runs for, as a screen lock
	does, and gets *no* for any other name, there or not.

*passwd* _name_ _current_ _new_
	Sets _name_'s password to _new_. Root changes anyone's, with an
	empty _current_; a user changes only their own, given the one it
	has.

*add* _name_ _full-name_ *admin*|*user* _password_
	Makes an account (below). Only root may.

*needs-owner*
	*ok yes* if no account can be signed in to yet, which is the case
	while there is no person's account (a number from 1000) and root's
	is locked, with no password line or one authd cannot read; *ok no*
	if one can.

*owner* _name_ _full-name_ _password_
	Makes the owner's account, an administrator's, at a machine's first
	sign-in. Only root may, and only while *needs-owner* says *yes*.

*reset* _name_ _password_
	Sets administrator _name_'s password to _password_, which may be
	empty, from the startup-trouble menu of *login*(1) (ADR 0253). Only
	root may, and only while the kernel was started with
	*startup-trouble* (*dukovo-boot*(7)); an account that is not an
	administrator's is refused, as an administrator changes its
	password with *elevate passwd*. The wrong passwords given for
	_name_ are forgotten, so the new one opens the account at once, and
	a disabled account is enabled. It is a security event, and each
	administrator is told at their next sign-in (see SESSIONS).

*disable* _name_
	Disables the person's account _name_ (ADR 0220): marks it with *!*
	before its hash, as Linux's *usermod -L*, keeping the password;
	ends the commands authd started for it through *elevate*(8) and
	forgets the panes unlocked for it; and has the kernel end the user
	(*radix-names*(2)'s *end*), again until no process of it is left.
	Its sessions and everything it runs end, and a directory it handed
	to another program opens nothing more. Answers *ok* and how many of
	its processes ended. Only root may. A disabled account's right
	password is answered *no the account* _name_ *is disabled*, which
	only someone who knows it learns; a wrong one as any wrong one.
	It refuses the only administrator whose account is enabled, whom
	Rescue and recovery need: *no that would leave no administrator*
	(*ACC-102*). So do *remove*, and *set* when it takes the right away.

*enable* _name_
	Takes the mark away: the account signs in again with the password
	it had. Only root may.

*remove* _name_
	Disables and ends the account as *disable* does, adds its number to
	the retired ones, which no account gets again, and takes its user,
	its own group and its name in other groups out of the account files.
	Answers *ok* and its home, which authd leaves as it is: *user*(8)
	keeps, archives or deletes it. Only root may. A service's account
	(*service*, below) is removed the same way.

*service* _name_ _bundle_ _home_
	Makes the account of a bundle's service (ADR 0385), as *bundled*(8)
	asks while it installs the bundle: the lowest number from 100 to 999
	that no user or group has and no removed account had, a group of
	its own, a password entry that is disabled, so nothing signs in to
	it, the shell _/system/commands/false_, the full name *A service of*
	_bundle_, and _home_, its data folder, which must be in
	_/shared/services_ and which bundled makes. An account of that name
	with that home is taken up again. Answers *ok* and its number. Only
	root may.

*socket-group* _name_ _bundle_
	Makes the group of a socket that _bundle_ offers to others (ADR
	0477), as *bundled*(8) asks while it installs the bundle: _name_ is
	the name of the account of one of its services and *-socket*, which
	must be a service account of _bundle_'s. The group has the lowest
	number from 100 to 999 that no user or group has and no removed
	account had, and no members. A group of that name with a number of
	that range is taken up again; one with a person's number, or an
	account of that name, is refused. The group goes when the account
	of its service is removed. Answers *ok* and its number. Only root
	may.

*rename* _name_ _new_
	Renames the account, its own group, its name in other groups, its
	hash's line and its notices, and moves its home if it is
	_/users/_*name*. Refused while any process acts for it, as
	_/processes_ shows. Only root may.

*set* _name_ [*full-name* _text_] [*admin* *yes*|*no*] [*console* *yes*|*no*]
	Changes the account's full name, whether it is an administrator's,
	or whether it is in the group *console*, whose members sign in to
	the web console without being administrators (WEB CONSOLE). Root
	changes any; anyone changes their own full name, as Linux's chfn
	lets them. Someone made no longer an administrator keeps nothing of
	the admin set: their elevated commands end and their panes lock.
	Someone who may no longer sign in to the web console is signed out
	of it.

*session* _name_ _password_ _term_ [_command_]
	Starts _name_'s session, if _password_ is theirs, on the terminal
	sent with the request as a descriptor (*SCM_RIGHTS*), with *TERM*
	set to _term_. With a _command_, the session runs it with the shell's
	*-c*, as *login --session --command* does, and ends with it: *login*(1)
	sends *report failure* when an administrator asks at the failure
	notice to report the failed start. A request without a _command_
	starts the person's shell. Only root may, as *login*(1) runs. A wrong password
	waits as *check*'s does. The answer is *ok* _process_, the number
	of the session's first process, and the connection stays open: when
	that process ends, authd writes *ended* _status_, its wait status,
	and closes it. See SESSIONS.

*elevate* _mode_ _password-kind_ _password_ _term_ _directory_ [_language_...] *--* [_command_...]
	Runs _command_ as an administrator, or root's shell for the
	_mode_ *shell* or *login* (*command* runs the command), in
	_directory_, for an administrator, after their password unless
	their pane is unlocked: _password-kind_ is *password*, with the
	password after it, or *none* or *ask*, with an empty field; *ask*
	lets consoled ask for it on a console (PROMPTS). Each
	_language_ is a setting *NAME=value* to keep. The request comes
	with four descriptors: the command's standard input, output and
	error, and the read end of a pipe. The answer is *ok password*
	when a password is needed and none came, or *ok* _process_, after
	which the connection stays open as a session's. See ELEVATE.

*unlock* _password-kind_ _password_
	Unlocks the asker's pane for 15 minutes, or 15 more, after the
	password unless it is unlocked: *ok*, or *ok password*.

*forget*
	Locks the asker's pane again. Anyone may ask.

*prompts*
	Asks for the passwords to be asked on the consoles: *ok*, then the
	connection stays open for *ask* and *done* lines (PROMPTS). Only
	root may, as *consoled*(8) runs; a later one takes over.

*prompted* _number_ *password*|*once*|*always*|*never*|*cancelled*|*elsewhere* _password_
	What came of prompt _number_: the password typed, what the person
	chose at a question, a prompt closed, or a terminal that is not a
	console's, with an empty _password_ for all but the first. Only the
	process that asked *prompts* may.

*choose* _terminal_ _name_ *permission*|*warning*|*grant* _text_
	Asks _name_, on the console of _terminal_, a question of
	*bundled*(8)'s about an app's permission (ADR 0224), whose words
	are _text_, one line: *ok* and what they chose, *ok cancelled*, or
	*ok elsewhere* when the terminal is not a console's or nothing asks
	on the consoles. Only root may, as bundled runs. When _name_ is
	root's and _terminal_ is that of a command authd elevated from a
	pane, the question goes to that pane instead, for the person who
	elevated it, its words beginning *As root, through elevate:*, and
	the audit log says so (ADR 0493).

*share-password* _name_ _current_ *set*|*clear* _new_
	Sets _name_'s sharing password to _new_, or clears it, which ends
	their share sessions (ADR 0228). Root sets anyone's, with an empty
	_current_; a person sets only their own, given their login
	password, which a wrong one waits for as *check*'s does. An empty
	_new_ is refused, and so is an account that is not a person's.

*share-signin* _from_ _negotiate_ _challenge_ _authenticate_
	A PC signs in to the shares from the address _from_: the three
	NTLM messages, in hexadecimal, as the PC sent them and *smbd*(8)
	answered. The request comes with three descriptors: the read end of
	a pipe, the PC's connection, and smbd's log. authd checks NTLMv2
	against the sharing password of the account the PC names, in any
	case, and starts that person's share session (SESSIONS), then
	answers *ok* _name_ _key_, the session key in hexadecimal. A wrong
	password, or a name with no sharing password, waits and is answered
	as *check*'s, by name and address. Only root may, as smbd runs.

*end-shares*
	Ends every share session, as *share off* does: *ok* and how many.
	Only root may.

*nfs-worker* _number_
	Starts *nfsd --worker* as the person numbered _number_, with their
	groups, if it is a person's account and not disabled, or as no one
	(65534) for any other number: the worker that serves an NFS client's
	calls as them (*nfsd*(8), ADR 0343). The request comes with three
	descriptors: the read end of nfsd's pipe to the worker, the write
	end of the pipe back, and nfsd's log. The worker sees the folders of
	the shares that name networks in _/system/settings/shares.toml_,
	each at _/.nfs-_*key*, and nothing else of the system. authd
	answers *ok* and the name it serves as, and notes the start in the
	audit log. Only root may, as nfsd runs.

*ssh-key* _name_ _key_
	Whether the key _key_, in hexadecimal as SSH sends a public key,
	may sign _name_ in over SSH (*sshd*(8), ADR 0347): their account is
	a person's and not disabled, the key is of a kind authd checks
	(*ssh-ed25519*, *ecdsa-sha2-nistp256* or *ecdsa-sha2-nistp384*),
	and their _~/.ssh/authorized_keys_ names it, read as sshd(8) says.
	*ok* if it may. sshd asks before a client signs with the key; only
	root may.

*ssh-password* _connection_ _name_ _password_
	Whether _password_ is _name_'s, for an SSH client at _connection_,
	once *sshd_config*(5) lets passwords sign in (ADR 0348). A wrong one
	is answered as for *check*, but counted for the client's address
	(WRONG PASSWORDS, BLOCKING) and recorded as *failed* on the terminal
	*ssh*, from it (SIGN-IN RECORDS). A password from an address that is
	blocked is not read: authd answers *no* and the reason in BLOCKING,
	which ends the connection. An empty password is refused. A right one, of
	a person's account that is not disabled, is answered *ok* and a
	ticket: 32 random bytes, in hexadecimal, that start _name_'s
	sessions on that connection, asked by the process that asked for
	it alone, until it says *ssh-ended*; sshd keeps no password. authd
	keeps 128 tickets at most, and lets the oldest go first. Only root
	may, as sshd runs.

*ssh-session* _connection_ _name_ *key*|*ticket* _proof_ _signature_ _term_ _kind_ _what_ [_language_...]
	Starts the session of someone who signed in over SSH. With *key*,
	_proof_ is what their client signed and _signature_ its
	signature, in hexadecimal (RFC 4252 section 7): authd checks the
	signature against the key the signed request names, that the
	request names _name_, and that the key may sign them in, as for
	*ssh-key*. With *ticket*, _proof_ is a ticket and _signature_ is
	empty: authd checks that it gave the ticket for _name_'s password
	to this connection and this process, and that
	the account is still a person's and not disabled. Then it starts
	_name_'s session (SESSIONS): with the _kind_ *shell* and no _what_,
	their shell; with *command*, *login --session --command* _what_
	(*login*(1)); with *subsystem* and *sftp*, the same with OpenSSH's
	_/system/libraries/ssh/sftp-server_, for SFTP and *scp*; with
	*forward* and no _what_, the connection's forwarder,
	*sshd --forward*, with no shell, and the network of a session but
	nothing else of one's (*sshd*(8), ADR 0349). The request comes
	with the session's terminal, of the type _term_, or with three
	descriptors, the session's standard input, output and error, which
	a forwarder needs. _connection_ is the four words of *SSH_CONNECTION*: the
	client's address and port, then this computer's. The session gets
	*SSH_CLIENT*, *SSH_CONNECTION*, on a terminal *SSH_TTY* and *TERM*,
	and each _language_ that is a *LANG*, *LANGUAGE* or *LC_\** setting.
	authd answers *ok* and the session's first process, and later
	*ended* _status_ on the same connection when that process ends, as
	for *session*. A refusal of a signature, a key or a ticket is noted
	in the audit log, and so is each session started, with the key's
	fingerprint or *a password*. Only root may, as sshd runs; at most
	32 SSH sessions run at once.

*ssh-ended* _ticket_
	The connection given _ticket_ ended: the ticket goes. Only the
	process it was given to may say so.

*ssh-failed* _connection_ _name_
	An SSH connection ended that tried to sign in, was refused every
	time, and never signed in: one failed sign-in for the client's
	address (BLOCKING). _name_ is the account it last tried, or empty.
	sshd says it once, as the connection ends, and not when authd
	counted a password of it already. Only root may, as sshd runs.

*blocked*
	The addresses blocked now, for *firewall blocked* (BLOCKING): *ok*
	and for each its address, when it ends in seconds since 1970, how
	many blocks in a row it has had, and why, a tab between them. Only
	an administrator may, inside *elevate*(8).

*unblock* _address_
	Lifts the block of an IPv4 address or an IPv6 network of 64 bits, an
	IPv6 address standing for its network, and forgets the blocks it had
	before; *ok* and the address as authd names it, or *no* if it is not
	blocked. Only an administrator may, inside *elevate*.

*never-block* _network_ [*remove*]
	Adds _network_ to the *never* list of *blocking.toml*(5), or takes it
	out with *remove*, and keeps the rest of the file; *ok* and the
	network, or *no* if it is in the list already, or is not in it. Only
	an administrator may, inside *elevate*.

*help-start* _minutes_ _key_ _open-ssh_ _term_ _password_
	Starts a help session (HELP SESSIONS) for the asker, a person, for
	_minutes_, 5 to 480, with their shell on the terminal that comes
	with the request, and the ends of two pipes that come with it, which
	sshd is given: the one it reads the help screen from and the one it
	writes the helper's typing to. _key_ is a public key as *ssh-key* gives it, or empty
	for a code. _open-ssh_ is *open-ssh* or empty; with it, and *sshd*
	off, an administrator's _password_ is checked and sshd is started for
	help sign-ins alone. _term_ is the asker's terminal type. authd
	answers *ok*, the session's ID, when it ends in seconds since 1970,
	its code, or *-* with a key, and sshd's host key fingerprint, or *-*
	when sshd made none, and then keeps the connection open to write
	event lines on, until the session ends. Refused for root, for a
	process in a help screen, for someone who has a session already, and
	when sshd is off and *open-ssh* is not given.

*help-end*
	Ends the asker's help session. *ok*.

*help-log* [_id_]
	Without _id_, the transcripts kept for the asker, or for everyone when
	root asks, inside *elevate*(8): *ok* and, for each, its ID, owner,
	start and end in seconds since 1970 (*0* for a session that has not
	ended), separated by spaces, the sessions by tabs. With _id_, that
	transcript's lines, separated by tabs, which only its owner and root
	may ask for.

*help-allow-elevate* _password_
	Lets *elevate* run in the asker's help screen for 15 minutes, once
	_password_ is checked as for *check*. Only an administrator, and not
	from inside the help screen, may. *ok* and the minutes.

*ssh-help-key* _key_
	Whether a help session waits for a helper with the key _key_, for
	sshd to ask before a client signs with it. *ok* if one does. Only
	root may, as sshd runs.

*ssh-help* _connection_ *code* _code_
*ssh-help* _connection_ *key* _signed_ _signature_
	Signs a helper in to the help session whose code _code_ is, typed as
	a person types it, or whose key signed the request, checked as for
	*ssh-session*. *ok* and the session's ID, with four descriptors: the
	help screen's two pipes' ends, for sshd to relay to and from the
	helper; the read end of a pipe of authd's, whose end tells sshd the
	session ended; and the transcript, to write what they type in. A wrong code is counted and
	answered as a wrong password is; an address that is blocked is
	refused unread. Only root may, as sshd runs.

*ssh-help-ended* _id_
	The helper's connection to session _id_ ended, which ends the session.
	Only the sshd process that signed them in may say so.

*keyring-set* _name_ _secret_, *keyring-get* _name_, *keyring-list*, *keyring-remove* _name_
	Keep _secret_, given in hexadecimal, as the entry _name_ of the
	asker's own keyring, answer *ok* and an entry's secret in
	hexadecimal, *ok* and the entries' names separated by tabs, or
	remove one (see KEYRINGS). Anyone may, of their own keyring, while
	it is open.

*keyring-unlock* _password_, *keyring-lock*
	Open the asker's keyring with their password, which waits after a
	wrong one as a sign-in does, or close it now.

*console* _verb_ _field_...
	A request of the web console (ADR 0514): *begin*, *trusted*, *code*,
	*challenge*, *passkey*, *start*, *unlock*, *extend*, *lock*, *end*,
	*elevate*, *token*, *token-start*, *terminal*, *follow* and
	*follow-end*, which only *admind*(8) asks, as root, and *steps*,
	*app-begin*, *app-confirm*, *app-remove*,
	*passkey-begin*, *passkey-add*, *passkey-remove*, *passkey-rename*,
	*codes*, *trusted-forget*, *token-add* and *token-remove*, which a
	person asks about their own account, from a process that runs as
	them. Binary fields, such as a ticket or a signature, are in
	hexadecimal. WEB CONSOLE says what each does.

authd knows who asks by the connection: the kernel gives it the process
at the other end, and the user and group it acted for when it connected
(*SO_PEERCRED*, *radix-posix*(7)). An administrator is root inside
*elevate*(8): see ELEVATE.

# KEYRINGS

Each person has a keyring (ADR 0393, *keyring*(1)): secrets such as
tokens and passphrases, kept in a file of authd's own, in which nothing
is in the clear. Its entries are sealed with ChaCha20-Poly1305 under a
random key of its own, and that key under one made from the person's
password with Argon2id, with a salt of its own, at their password
hash's cost.

authd opens a keyring when its person signs in with their password, at
a console or over SSH, or asks *keyring-unlock*, and makes it then if
they have none; an account with no password has none. It holds it open
in its own memory while one of their sessions runs, and closes it,
wiping the key, when the last one ends. A sign-in to the web console
(WEB CONSOLE) opens none.

A change of password seals the keyring's key under the new one. When
root sets a password without the old one, or the startup-trouble menu
resets it, a keyring that is not open is emptied, since only the old
password opened it, and the person is told; one whose new password is
empty is emptied too. A removed account's keyring goes with it.

authd says when a keyring opens and closes, and why one did not:

```
authd: keyring of alice: open
authd: keyring of alice: closed
```

# JOBS

authd runs the jobs of people for *timerd*(8) (ADR 0478): the lines of
a crontab (*crontab*(5)) and the jobs of *at*(1). It starts each as the
person and their groups, as it starts a session's first program, in a
view of theirs with their own _/tmp_ and a session's network:
*sh -c* with the command, in the folder the job names (the home for a
crontab line, the folder *at* ran in for an *at* job), with the null
device as its standard input and the descriptor timerd sent as its
standard output and error. Its environment is a session's, without
*TERM*, with the job's own settings over it. authd tells timerd when it
ends, as it tells a sign-in program. Only timerd, which is root, asks.
authd runs 16 jobs at once, and timerd starts no more: one whose time
comes while 16 run waits for one of them to end.

A job runs for a person who is signed in, at a console or over SSH; a
share's session, an elevated command or another job does not count.
For one who is not, it runs only if they asked for that with
*crontab --keep-running on* (*crontab*(1)), or chose nothing and the
system's default says so, and no administrator barred it with
*user set --jobs-signed-out never* (*user*(8)). The default is off
unless _/system/settings/jobs.toml_ holds *keep_running = true*, as the
*server* preset writes. Such a job has no keyring, which opens with the
person's password. Root's jobs always run. A job of a disabled account,
or of one that is no person's, does not run.

A job that does not run because its owner is signed out is counted for
them, and their next sign-in says how many did not, after the notices:

```
2 scheduled jobs of yours did not run while you were signed out, the first on Sat Oct 31 02:00 2026. crontab --keep-running on lets them run then.
```

Each job's start and end is a security event, as a session's is:

```
authd: alice's job crontab alice, for process 40: process 71, while they are signed out
authd: alice's job crontab alice, process 71, ended with status 0x0
```

The setting is kept for the person's account, and read and written by the
person themselves (*crontab --keep-running*) or by an administrator
(*user set --jobs-signed-out*).

# WRONG PASSWORDS

A wrong password is answered only after a wait: 2 seconds for each of
the first three in a row, then twice as long each time, up to a minute,
for that account asked by that user, or, for a Windows PC's sign-in to
the shares, an SSH client's password and a browser at the web console,
from that address. A try made while one waits waits
its turn, so tries at once do not go faster. A right password ends the
run; one forgotten after 15 minutes with no try ends it too. A name
there is not is checked against a hash all the same, and answered the
same way after the same wait, so that neither the answer nor the time
says whether the account is there.

authd answers 64 connections at a time, 4 from one process and 32 from a
process that runs as root, as admind does, which holds one for each
command of the console's that runs; one that sends no whole request in
10 seconds is closed.

The wait slows a guesser who keeps to one account or one address. An
address that guesses at many accounts is blocked: see BLOCKING.

# BLOCKING

authd counts each failed sign-in from the network for the address it
came from, whatever account it named, and blocks an address that keeps
failing (ADR 0479). A failure is a wrong password over SSH, a wrong
sharing password, a wrong help-session code (*help-session*(1)), a wrong
password, second-step code or API token at the web console (*admind*(8)
gives the browser's address), or an SSH connection that tried to sign
in, was refused every time, and ended (*ssh-failed*). An address is an IPv4
address or an IPv6 network of 64 bits, since one computer holds a whole
/64; an IPv4 address written as IPv6 is the IPv4 one.

By default, 5 failures from an address within 10 minutes block it for 10
minutes, and each block that comes within a day of the end of the one
before lasts twice as long, up to a day. A right password from an
address clears its count, but not its record of blocks; at the web
console the whole sign-in must be right, with its second step, so that
someone who knows a password and guesses at its codes is still counted.
The count is in
memory only, and is bounded: past 4096 addresses authd forgets the
oldest.

*blocking.toml*(5) sets these numbers, turns blocking off, and names the
networks that are never blocked. authd looks at the file's time of
change and length before each request it answers, and reads it again if
either changed, so a change needs no restart; a file it cannot read is
logged with its reason and the settings stay as they were. Never
blocked, besides those networks, are loopback, this computer's own
addresses, and the address an administrator is signed in from over SSH
or at the web console while their session lasts, so that a mistyped password cannot lock out
the person who is fixing the computer. The console and the serial port
are not on the network, so they are never blocked.

A block goes to the kernel's firewall (*radix-firewall*(2)
*set_blocked*) as the whole list each time it changes: the address, the
seconds it has left, and the ports it closes, which are those *sshd*,
*smbd* and *admind* listen on, as their service files say, or port 22
when none does. The firewall drops what the address starts on those
ports before any rule, and no more: the address still reaches every
other port. The kernel ends a block when its time is up, even if authd
is late. A block does not end a connection that is open, but authd
answers its next password with "this address is blocked for guessing
passwords for" and how long is left, without reading the password, and
*sshd* ends the connection. The same reason refuses a sharing sign-in.

The blocks are kept in _/system/state/authd/blocked_, mode 0600, so
that they outlast a restart: authd gives the kernel those whose time is
not up as it starts. A block is kept for a day after it ends, so that
the next one lasts longer.

Each block, each unblock and each change of the settings is a line of
the audit log, and each administrator's next sign-in is told how many
addresses were blocked since their last (see SESSIONS):

```
authd: blocked 203.0.113.9 for 10 minutes: 5 wrong passwords over SSH in 2 minutes, for root, admin and test
authd: unblocked 203.0.113.9, for process 412 (user 0)
authd: 192.168.1.0/24 is never blocked, for process 413 (user 0)
```

*firewall blocked* lists the blocks, *firewall unblock* lifts one, and
*firewall never-block* changes the networks that never are
(*firewall*(8)).

# HELP SESSIONS

A help session lets a helper who has no account sign in over SSH as
*help* and work in a shell of the owner's, on the owner's screen, which
the owner watches and ends with one key (ADR 0540). *help-session*(1)
asks for it with *help-start*, sending the help screen's terminal and the
ends of two pipes for sshd.

authd makes a resource domain *help-*_id_ below its own, which Spark gives
it a capability to because its service file asks for one (*domain =
true*, *service.toml*(5)), starts the owner's login shell on the terminal
in that domain, as the owner, with *HELP_SESSION*=_id_, and makes the
transcript. The help screen's shell is a session as SESSIONS describes,
but for its domain. A session waits for its helper with a code, made of 16
letters and digits with no *I*, *O*, *0* or *1*, used once, or with the
key the owner named. While one waits for a code, authd keeps
_/system/live/authd/help-waiting_ in place, so that *sshd*(8) offers
passwords for *help*. When the helper signs in, authd gives sshd the
socket's other end and the transcript, and tells the owner's connection.

Each wrong code is counted for the address it came from, as a wrong
password is (BLOCKING), and answered as one is. Five of them spend the
code, for every session that waited for one, which tells its owner; a
session that has no key to wait for then ends.

authd writes event lines on the connection that *help-start* left open,
each ended by a newline: *helper* _address_ _key_ when a helper signs in,
_key_ being their key's fingerprint or *code*; *left* _minutes_ at 5 and
1 minutes before the end; *elevate* _minutes_ when the owner allows
elevate; *spent*; and last *ended* _why_, which is *owner*, *deadline*,
*helper* (their connection closed), *session* (the shell ended), *spent*
or *refused*, after which it closes the connection. When the connection
closes first, authd ends the session.

A session ends when its owner asks (*help-end*, or the key Ctrl-] in
*help-session*), when its time is up, when the helper's connection or the
shell ends, and when authd or *sshd* restarts. authd then stops the domain,
which ends every process the helper started, closes its pipe to sshd,
which ends the helper's connection, writes the end to the transcript and to the
audit log. What the helper changed in the owner's files stays.

Nothing in a help screen asks for a password, since the helper could draw
the question and read the answer. authd tells a process of the screen by
its place in the session's domain, and refuses what it asks for there,
except from a process that is root, as an elevated command is:
*check*, *unlock*, *passwd*, *share-password*, *help-start* and
*help-allow-elevate* answer
"in a help session, nothing asks for your password: run it from another
terminal", and the keyring's requests "in a help session, the keyring
stays shut: use it from another terminal". *elevate* is refused with
"in a help session, elevate runs only while its owner allows it:
help-session allow-elevate". After *help-allow-elevate*, which asks the
password outside the screen, *elevate* in the screen runs for 15
minutes with no password, and its command runs in the session's domain,
so that the end stops it too.

The transcript, _/system/state/help/_*id*_.log_, is mode 0600 and root's,
and holds a line for each step: *start*, *helper*, *typed*, *elevate*,
*spent* and *end*, as *help-session*(1) describes. sshd writes the
helper's lines, and authd the others. The 20 newest of each owner are
kept; an older one is removed as a new session starts. Each step is also a
line of the audit log:

```
authd: alice started help session k7q2mx until 15:04, for a helper with a code: shell process 412
authd: a helper signed in to alice's help session k7q2mx from 192.168.1.30 with code, for process 431
authd: a wrong help-session code from 192.168.1.30, for process 433
authd: help session k7q2mx of alice ended (owner); the helper from 192.168.1.30 with code changed what the transcript shows, which stays
```

# RESCUE

*dukovo-rescue*(8) starts authd in Rescue when the installed system had SSH
on, so that its administrators sign in over SSH to repair it (ADR
0540). Rescue's live system has accounts of its own, but authd then signs
people in by the installed system's: it takes the users, groups, password
hashes and keys from the root Rescue mounted, through the link
_/system/live/rescue/root_, which dukovo-rescue makes in a folder that only
root writes. A system started without *rescue=* on the kernel's command
line, or a link that is not root's, reads nothing there.

In Rescue authd answers *ssh-key*, *ssh-password*, *ssh-session*,
*ssh-failed*, *ssh-ended*, *blocked* and *unblock*, and refuses every other
request with "Rescue changes no account: authd answers only SSH's
sign-ins here", the help requests among them. It signs in only the
root's administrators, a member of _admins_ other than root, and refuses
anyone else with "only this computer's administrators sign in to Rescue".
It writes nothing to the root: no sign-in record, no notice, no change of
an account; its blocks and counts are in Rescue's own memory, and its audit
lines go to Rescue's log. The sign-in rules over SSH are *sshd*(8)'s, by the
root's _sshd_config_.

An administrator's session over SSH is not their shell. authd starts
*dukovo-rescue --ssh* _name_ on their terminal, as root, as an elevated
command runs, with *SSH_CONNECTION* set and no password asked again; a
command, SFTP or a forwarder is refused. The menu says who signed in, and
its choice to open a shell is the shell.

# SESSIONS

A session is the processes of someone who signed in, acting for their
user and groups (*radix-names*(2)), on the terminal they signed in at
(ADR 0250). authd starts its first process itself, as its own child,
with the *names* capability that Spark gives it because its service
file asks for it (*names = true*, *service.toml*(5)), and that no other
service holds:

- it stamps a copy of its root for the user and their groups, and makes
  one without the bypass right or the control right from that, the
  session's root, so that the session mounts and unmounts nothing;
- it opens _/system/commands/login_ through that root, so that it is a
  program the user may run;
- unless the user is root, it builds a view (ADR 0215) acting for them,
  as the session's root in place of that copy: each name of the root at
  its place, with the user's own _/tmp_ at _tmp_ (see below);
- it makes a process, names it for the user and their groups, and gives
  it that root, the random source, the log, the terminal as its
  descriptors 0, 1 and 2, the network and the jit capability (see
  below), and nothing else of authd's: not its root, not the names
  capability, not the bypass right;
- it runs *login --session* in it, with the environment *HOME*,
  *SHELL*, *USER*, *LOGNAME*, *PATH=/system/commands*, *TERM*,
  *LANG=C.UTF-8* and *EDITOR=nano*; login makes the terminal the
  controlling terminal of a new session and runs the user's shell in
  their home.

Each person has a _/tmp_ of their own (ADR 0255),
_/system/live/authd/tmp/_*number*, theirs and mode 1777, which authd
makes at their first sign-in; their sessions share it, so tmux and
screen find their servers from any of them, and no one else's view
holds it. It lasts until the computer stops, and removing the account
removes it. Each place of the view is attached without the bypass
right and without control, so the owners and modes of the files decide
as they would without it, and nothing is mounted or attached in it. A
name added to the root after a session started is not in its view
until the next sign-in. Root's sessions, and commands run with
*elevate*(8), keep the whole tree and the system's _/tmp_; root finds a
person's at its real path.

The network comes from the capabilities that authd's service file asks
Spark for (*network.outbound*, *network.listen* and *admin.network*,
ADR 0257). A session connects out, and listens on the ports from 1024,
as anyone may on Linux, but sets up no interface and listens on no port
below 1024: binding one fails with *EACCES*, as on Linux. Root's
sessions, and commands run with *elevate*(8), get the network as authd
holds it, so *ip*(8) sets the interfaces up and a server listens on
port 80.

The jit capability (*radix-jit*(2), ADR 0624) comes from Spark too,
because authd's service file asks for *memory.jit*. authd puts it in
_SLOT_JIT_ of each session, help screen, web terminal, SSH session, job
and elevated command, with the copy and transfer rights, so that their
programs keep it through *fork* and *exec*. Holding it gives no program
memory that is writable and executable: *bundle*(1)'s *run* uses it to
give memory.jit to an app whose bundle was granted it, and *linux*(1)'s
*run --allow-jit* to the Linux program it starts, and neither puts it in
the sandbox. A share session's, an NFS worker's and the web console's
workers do not get it.

The groups are the account's own and each group of
_/system/settings/group_ that lists its name, up to 32. A session
started, and a session ended, with the wait status of its first
process, are security events:

```
authd: alice signed in on pts/0: session process 63, for process 58 (user 0)
authd: alice signed out on pts/0: session process 63 ended with status 0x0
```

The sessions' processes run in authd's resource domain, as the shells
ran in *consoled*(8)'s before sign-in, so its service file sets no
memory limit. A process authd makes ends when authd does, so a restart
of authd ends the sessions it started.

Without the names capability authd logs *no names capability: sign-in
cannot start sessions* when it starts, and refuses *session*.

A share session is a Windows PC's, signed in through *smbd*(8) (ADR
0228): authd starts *smbd --worker* as the person, as it starts a
session's first process, in a view of theirs, with the pipe smbd sent
as its descriptor 0, smbd's log as 1 and 2 and the PC's connection as
3, and no network of its own. It is on the terminal *smb*, from the
PC's address, and ends with the connection, when the person's sharing
password is cleared, or at *end-shares*.

Before a session's programs start, authd writes on its terminal when
the person last signed in, and the wrong passwords typed for them since
(see SIGN-IN RECORDS), as Linux's pam_lastlog does:

```
Last sign-in: Sat Oct 31 12:40 2026 on pts/0
Since then, 1 wrong password was typed for bob, the last on Sat Oct 31 12:44 2026 on pts/1.
```

Then what the person is to be told since they last signed in, which
authd forgets once the session starts: today, that a password was reset
from the startup-trouble menu, which every administrator is told:

```
authd: the password of alice was reset from the startup-trouble menu, by process 61 (user 0)
```

```
Your password was reset from the startup-trouble menu on Sat Oct 31 12:40 2026.
The password of alice was reset from the startup-trouble menu on Sat Oct 31 12:40 2026.
```

# SIGN-IN RECORDS

authd records the sign-ins in _/system/state/sign-ins_, for *last*(1),
*lastb*(1) and *lastlog*(8) (ADR 0254), a line of text each: the time
in seconds since the epoch, what happened, and its fields, *-* for
none:

```
1793440000 boot 1.0.0-alpha.1
1793440100 in alice pts/0 - 61
1793440200 failed bob pts/1 -
1793440300 out alice pts/0 - 61
1793440400 down
```

- *boot*: the computer started, with the kernel's release. authd
  records it as it starts, once for each start of the computer, with
  the kernel's time of the start, and notes that time in
  _/system/live/authd/boot_, which *last*(1) tells the start that runs
  by: a time service that sets the clock after the start moves the
  start's time as the clock tells it, and the note stays.
- *down*: authd was asked to end, as Spark asks each service when the
  computer stops. The sessions end with it.
- *in* and *out*: someone signed in, or their session's first process
  ended, with the terminal, where from (*-* at the computer itself),
  and that process, which pairs them.
- *failed*: a wrong password was typed at a sign-in, with the name
  typed.

A web console session (WEB CONSOLE) is recorded on the terminal *web*,
from the browser's address, with its worker's process as the session,
and a wrong password or app code at its sign-in as *failed* there. A
person's own shell in the console's Terminal is recorded on its terminal,
from the browser's address, as an SSH session's is. A
share session (SESSIONS) is recorded on the terminal *smb*, from the
PC's address, and a wrong sharing password as *failed* there. An SSH
session on a terminal is recorded on it, from the client's address; one
without a terminal is not, as on Linux. A wrong password over SSH is
recorded as *failed* on the terminal *ssh*, from the client's address; a
key that does not sign in is not, as Linux's sshd records only
passwords.

The starts and stops are in _system_, which everyone reads. Each
account's sign-ins, sign-outs and wrong passwords are in a file named
by its number, root's and the account's own group's, mode 0640, so the
person and root read it. The wrong passwords typed for names that are
no account's are in _unknown_, root's alone. The directory is 0711, so
no one but root lists it. A file that grew past 512 KiB keeps its
newest 4096 lines, and its newest sign-in with its sign-out and its
newest start if they are older.

# WEB CONSOLE

*admind*(8) is the web console's server (ADR 0514), and holds no secret:
authd checks each password, code, passkey and token, decides who signs
in and what a session may do, starts each session's worker and runs the
commands that an administrator's session runs. The requests are the
*console* ones (THE SOCKET). admind's, which only root may send, are
refused to anyone else; a person's own, about their own account, are
answered for the user the connection gives.

*begin* _address_ _name_ _password_
	Checks the password of _name_ as *check* does, for a browser at
	_address_: the wait after a wrong one is counted for the account
	from that address, and a wrong one is recorded (SIGN-IN RECORDS).
	Then it checks the account's right to the console, and refuses
	root, a disabled account, and one that is neither an administrator's
	nor in the group *console*, saying so only to someone who gave the
	right password. The answer is *ok done* _ticket_, when the sign-in needs no more, *ok
	step* _ticket_ _kinds_, when a second step comes, _kinds_ being
	*app*, *passkey* and *recovery*, joined by commas, or *ok needs-step*
	_ticket_, when the sign-in rules (*admin.toml*(5)) ask for a step the
	account has not got, so that its session may only add one. A ticket
	is 32 random bytes, in hexadecimal, that only the process it was given to
	may use, once, for 5 minutes; authd keeps 128, and lets the oldest go
	first.

*trusted* _ticket_ _token_, *code* _ticket_ _code_
	Give the second step of a sign-in: a browser's token, from "Trust
	this browser", which skips it, or a code. A code is the app's 6
	digits, taken from the step before, now or after, each step once
	(RFC 6238), or a recovery code, taken once; a wrong one waits as a
	wrong password does and is recorded.

*challenge* _purpose_ _value_ _relying-party_, *passkey* _purpose_ _value_ _answer_...
	A passkey's second step, a passkey alone and a passkey that unlocks
	a session (WebAuthn). *challenge* gives 32 random bytes to be signed,
	for 5 minutes and once, for the account's passkeys at that name;
	*passkey* checks the answer: that the challenge is the one given for
	this purpose, that the browser's page is the console's own, that
	the key is one of the account's for that name, its signature, and
	that its count rises unless it and the one kept are both 0. A
	passkey alone, and one that unlocks, must have checked the person
	(PIN, fingerprint or face). The keys are ES256, EdDSA and RS256, the
	last of 2048 to 4096 bits. A new passkey's attestation is not
	checked. A name that is an address is refused, since no browser
	makes a passkey for one. authd keeps 128 challenges.

*start* _ticket_ *trust*|*once* _browser_
	Starts the session of a sign-in that is done, once, and answers
	*ok* _process_ _session_, and, with *trust*, the token that the
	browser keeps for 30 days. The request comes with three
	descriptors: the pipe of admind's calls, the pipe of the replies,
	and the log. Pipes, since a socket cannot be sent (ADR 0091). authd
	starts the session's worker, *admind --worker*, as the person, with
	their groups and the network of a session and nothing else of one's,
	no shell, its 0 the calls' pipe, its 3 the replies' pipe and its 1
	and 2 the log, as a child of its own; the worker's calls to authd are the person's own requests. The
	session is 16 random bytes, in hexadecimal. A sign-in that needs a
	second step the account has not got starts a session that may only
	add one. authd keeps 64 sessions, and audits and records each
	(SIGN-IN RECORDS).

*unlock* _session_ *password*|*code*|*passkey* _proof_
	Unlocks the session for 15 minutes after the person proves again
	that it is them, with their password, an app's code or a passkey
	that checked them; a recovery code does not do. *extend* sets the
	end to 15 minutes from now, while it is unlocked; *lock* locks it at once;
	*end* ends the session, which the worker's end does too. A session
	past *session-hours* does not unlock, and an end by *idle-minutes*
	is admind's to say. Unlocked sessions are kept in memory only.

*elevate* *session* _session_|*token* _token_ _area_ [_language_...] *--* _command_...
	Runs _command_ as an administrator for an unlocked session, or for
	a token whose scopes name _area_, the console's area of the action
	that runs it, or are *\**, as *elevate* does for a pane (ELEVATE),
	with its four descriptors, and answers as it does. A scope that only
	reads, such as *services:read*, is never enough, and _area_ is never
	*\** itself. The
	account must be an administrator's, and one that may sign in. The
	command is found in _/system/commands_ and _/applications/commands_
	only, by a path with no *.*, *..* or empty part, and is never a
	shell (*nukesh*, *sh*, *bash*, *dash*, *zsh*, *ksh* or *fish*); a
	command has at most 64 arguments and 32 KiB. The command's
	environment, as *elevate*(8)'s, has *DUKOVO\_CONSOLE\_SESSION* too: the
	first 8 characters of the session's ID, or *token* and the token's
	name. The audit log says whose right ran it, and for a token, the
	area.

*token* _token_ _address_, *token-start* _token_
	Check an API token, which has the form dkv_ACCOUNT_SECRET, with
	SECRET 64 hexadecimal digits, and start a worker for its owner as
	*start* does. A wrong token waits as a wrong password does. admind
	asks *token* for each request that brings one, so that a token that
	is removed or ends, or whose account may no longer sign in, stops at
	once, and *token-start* only when an action runs as the token's
	owner. The answer to *token* is *ok* _owner_ _name_ _scopes_ _until_.

*terminal* _session_ *own*|*admin* _term_ [_language_...]
	Starts the shell of a pane of the console's Terminal (ADR 0517) on
	the terminal that comes with the request as its one descriptor,
	whose other side admind holds, as *sshd*(8) holds an SSH session's.
	*own* is the person's login shell, started as an SSH session's is,
	after their last sign-in and their notices, and recorded on that
	terminal from the browser's address (SIGN-IN RECORDS). *admin* is
	root's login shell, as *elevate -i* starts it, for an administrator
	whose session is unlocked; it is not recorded, as an elevated
	command is not. _term_ must be a terminal type the machine knows,
	and _language_ is the browser's language settings, as _NAME=value_.
	The session must be one that may do all that is a person's, and its
	account one that may still sign in. *admin.toml*(5)'s *[terminal]
	allowed* must let the account, which must have a shell, else the
	refusal starts with *ADM-132:*; and it may run at most four shells
	for each terminal *sessions-per-account* lets it keep (*ADM-130:*),
	so that admind shows the console's own ID. The answer is *ok*
	_process_, the shell's first process; authd does not hold the
	request, and the shell ends when admind closes the terminal or the
	shell exits. An administrator's shell also ends when its session
	locks, its unlock runs out or the session ends: authd looks every
	second while one runs, sends SIGHUP to its process group, and
	SIGKILL 5 seconds later to what is left of the group. The audit log
	has each shell's start and end, and why an administrator's ended.

*follow* _session_ *own*|*all* _service_ _kernel_ _priority_ _grep_, *follow-end* _session_ _process_
	Follow the log for the console's Logs page (ADR 0518): authd starts
	*log --output json --follow --lines 0* with the options it is given,
	on the two descriptors that come with the request, the write ends of
	the pipes admind reads the output and the errors from. authd checks
	the options as *log*(1) does and writes the command itself; admind
	sends no command, and nothing but *log* is started this way. An empty
	field is an option not given, and _kernel_ is *kernel* or empty.
	*own* runs as the person, and shows the records of their own
	programs, as *log* does at their terminal (ADR 0252). *all* runs as
	root, as *elevate log* does, for an administrator whose session is
	unlocked, with an audit line for its start and for a refusal; authd
	ends it when the session locks, its unlock runs out or it ends, as an
	administrator's shell (*terminal*) ends. A follow of either kind ends
	with its session. The answer is *ok* _process_, and authd does not
	hold the request. *follow-end* ends the follow _process_ of
	_session_, which admind no longer reads: SIGHUP, then SIGKILL 5
	seconds later; authd, which reaps it, signals it, so that the number
	is never another process's.

*steps*, *app-begin*, *app-confirm*, *app-remove*, *passkey-begin*, *passkey-add*, *passkey-remove*, *passkey-rename*, *codes*, *trusted-forget*, *token-add*, *token-remove*
	The person's own second steps and tokens (below). *app-begin*,
	*app-remove*, *passkey-begin*, *passkey-remove*, *codes* and
	*token-add* take the person's password as _proof_, or nothing, from
	the worker of their own console session while it is unlocked.
	*steps* lists them with none of their secrets.

*steps-of* _name_, *steps-reset* _name_, *token-remove-of* _name_ _token_
	Another person's second steps and tokens, as *user*(8) asks them
	(ADR 0562); only root may. *steps-of* answers as *steps* does.
	*steps-reset* removes the app, the passkeys, the recovery codes and
	the trusted browsers, keeps the tokens, tells the person at their
	next sign-in (see SESSIONS), and answers *ok* and how many of each
	went: the app (0 or 1), passkeys, recovery codes and trusted
	browsers. *token-remove-of* removes the token named _token_, or is
	refused with *ACC-104*. Each change is an audit line.

*end-session* _name_ _session_ *hang-up*|*kill*
	Sends SIGHUP, or SIGKILL, to each process of the session at the
	machine that process _session_ leads and that acts for the account
	_name_, for *user end* (ADR 0562); only root may. The session must be
	one that authd started for the account, at a terminal or over SSH,
	or it is refused with *ACC-104*: its processes are below authd, and
	the kernel lets a process signal only its own session's and those
	below it, so authd alone can end them. A hang-up tells the person at
	their next sign-in who ended the session. authd answers *ok* and how
	many processes the signal was sent to, and each is an audit line.

Each account's second steps and tokens are kept in one file,
_/system/state/authd/console/_*name*.toml, in a folder of mode 0700,
which only root reads and which no other service's view holds. authd
writes it anew beside the old and renames it, and an account that has
none has no file. It holds the app's secret (base32), each passkey's ID,
public key, name, count, and the name it was made for, recovery codes
as SHA-256 hashes, trusted browsers as SHA-256 hashes of their tokens
with their ends and descriptions, API tokens as SHA-256 hashes with their
scopes and ends, and the handle its passkeys carry. Removing the
account removes the file; renaming it moves the file and ends the
account's tokens.

An account has one authenticator app, 16 passkeys, 10 recovery codes,
16 trusted browsers and 32 tokens. An app is added with *app-begin*,
which makes the secret and keeps it for 10 minutes, and *app-confirm*,
which takes a code of it; the first second step an account adds also
makes the recovery codes. *codes* makes new ones, which end the old.
Removing the last step removes the codes. When the sign-in rules ask the
account for a second step, its last one is not removed. A token is made
with a name of 1 to 32 letters, digits and *-*, scopes, and 1 to 366 days,
and the answer shows it once.

A session, a sign-in under way, a challenge and an app being added are
in memory only: authd starting again ends them, and the workers, its
children, end with it. The sign-in rules are in
_/system/settings/admin.toml_, which authd reads at each request that
needs them. Each sign-in with how it was proved, each refusal, each wrong
proof, each unlock, end and command, and each change to a second step or
a token is in the audit log; no secret is.

# ELEVATE

*elevate*(8), and *sudo*(8), which is the same program, run a command as
an administrator through authd (ADR 0219). For an *elevate* request
authd:

- finds the asker's account by the user the connection gives, and
  refuses one that is not an administrator's, a member of the *admins*
  group, saying so in the audit log, before any password;
- finds the command in _/system/commands_ and _/applications/commands_
  only, or as given if it holds a */*, and refuses one not found;
- unless the asker's pane is unlocked, answers *ok password* to a
  request without a password, and checks the one that comes, waiting
  after a wrong one as for *check*;
- makes a process, its own child, acting for root as authd does, and
  gives it authd's root, which passes the owners and modes of files
  (the bypass right), the random source, the log, the platform
  capability that its service file's *admin.hardware* gives it, its
  network capabilities, and the three descriptors as its 0, 1 and 2:
  the admin set;
- runs *elevate --started* in it, which makes the first terminal among
  them its controlling terminal in a new session, goes to the
  directory and runs the command, with the environment *elevate*(8)
  describes;
- answers *ok* _process_, unlocks the pane for 15 minutes from then,
  and audits it.

A pane is the terminal of the asker's session, with that session and
when its leader started, as *radix-proc*(5) shows them: another session
on the same terminal is another pane, and a program with no terminal
has none, so it is asked each time. Unlocked panes are kept in memory
only, and forgotten when authd starts again.

Then authd reads the pipe: a line *signal* _number_ sends the command's
process group that signal, and *continue* sends it *SIGCONT*; the end
of the pipe, or of the connection, hangs the command up with *SIGHUP*,
as elevate is gone. When the command stops, authd writes *stopped*
_signal_ on the connection; when it ends, *ended* _status_, and closes
it. The audit lines:

```
authd: alice elevated on pts/0 (password typed): process 81 runs /system/commands/id, for process 77
authd: alice elevated on pts/0 (unlocked): process 84 runs /system/commands/user add bob, for process 77
authd: alice's elevated process 84 on pts/0 ended with status 0x0
authd: alice unlocked elevate on pts/0 for 15 minutes, for process 77
authd: bob, not an administrator, asked to elevate on pts/1: dmesg, for process 93
```

A command line shows as at most 256 characters, with *?* for each
control character. An elevated command runs in authd's resource domain
and ends when authd does.

# PROMPTS

*consoled*(8) asks *prompts* when it starts, so that the password for
elevate, in a pane on a console, is asked where no program draws or
reads (ADR 0251). An *elevate* or *unlock* request whose
_password-kind_ is *ask*, from a locked pane, is then not answered *ok
password*: authd keeps it, with its descriptors, and writes on
consoled's connection

```
ask 12 pts/0 alice /system/commands/user add bob
```

the prompt's number, the pane's terminal, whose password it is and the
command line, empty for *unlock*. consoled answers with a *prompted*
request: authd takes the password typed as the one the request came
with, waiting after a wrong one as ever, and elevate asks again; a
closed prompt is answered *no no password was typed*, and *elsewhere*,
for a terminal that is not a console's, *ok password*, so that elevate
asks itself. A prompt not answered in 5 minutes is given up. When its
elevate goes away first, authd writes *done* _number_, and consoled
takes the prompt off. While no program asks *prompts*, and when it
goes away, *ask* is answered as *none* is.

A *choose* request is passed on the same way,

```
choose 13 pts/0 alice permission Notes (org.example.notes) asks to: use your documents folder, to read and change it
```

and answered with what consoled says the person chose: *once*,
*always* or *never*, *cancelled*, or *elsewhere*. Without consoled, or
after 5 minutes, it is answered *elsewhere* or *cancelled*.

# ACCOUNTS

An account's name is 1 to 32 lowercase letters, digits, *\_*, *-* and
*.*, starting with a letter or *\_*, as Debian's adduser takes them. It
gets the lowest number from 1000 that no user or group has and no
removed account had, a group of
its own of that name and number, the shell *nukesh*(1) and the home
_/users/_*name*, of mode 0700, with _documents_, _downloads_, _desktop_,
_development_, _music_, _pictures_, _videos_ and _applications_, of mode
0755, and _app-data_, of mode 0700, all the user's. An administrator's
account is also in the *admins* group, number 10, which authd makes if
it is not there.

When it starts, authd makes the home of each account that has none,
such as one *ob image --user* made in an image before its first boot,
whose files are all root's. On a machine with no account, whose image
was made without *--user*, the first sign-in makes the owner's
(*login*(1)). It makes the folders while the home is
still root's, and gives the home to its user last, so that a user
already running cannot put a link where a folder is to go.

Each account made, disabled, enabled, removed, renamed or changed, each
password changed and each wrong password is a security event in the
audit log (*logd*(8)), with the process and user that asked. A name a request gave is shown as at most 64 characters,
with *?* for each control character, so that it cannot start a line of
its own:

```
authd: made the account alice (1000), an administrator's, for process 41 (user 0)
authd: a wrong password for alice, from process 52 (user 0); 1 in a row
```

# FILES

_/system/settings/passwd_
	The users.

_/system/settings/group_
	The groups.

_/system/state/authd/passwords_
	The passwords' hashes, a disabled account's with *!* in front.

_/system/state/authd/retired_
	The numbers of removed accounts, which no account gets again: a
	line _number_:_name_:_date_ each.

_/system/state/authd/notices_
	What each person is told at their next sign-in: a line
	_name_<tab>_text_ each.

_/system/state/authd/jobs_
	For each person whose jobs have a setting, a line _number_
	_chosen_ _bar_ _missed_ _first_: whether they keep their jobs
	running while signed out (*on*, *off* or *default*), whether an
	administrator barred it (*barred* or *allowed*), and how many did
	not run since they last signed in, the first at _first_, in seconds
	since 1970 (see JOBS). Only root reads it.

_/system/settings/jobs.toml_
	The system's default for jobs while their owner is signed out:
	*keep_running = true* or *false*. Without it, off.

_/system/settings/blocking.toml_
	When an address that guesses passwords is blocked, and the networks
	that never are (see BLOCKING).

_/system/state/authd/blocked_
	The blocks in force and those a day old at most: a line _address_
	_until_ _started_ _times_ _why_ each. Only root reads it.

_/system/state/help/_*id*_.log_
	A help session's transcript, mode 0600, root's (see HELP SESSIONS).

_/system/state/help/index_
	The transcripts kept, a line each. Only root reads it.

_/system/live/authd/help-waiting_
	Present while a help session waits for a helper with a code, so that
	*sshd*(8) offers passwords for *help*.

_/system/live/authd/help-only_
	Present while *sshd* runs for a help session alone, which
	*help-session start --open-ssh* started it for: it takes sign-ins as
	*help* and no other.

_/system/live/rescue/root_
	In Rescue, the link to the installed system's root, which authd reads
	its accounts through (see RESCUE).

_/system/state/authd/keyrings/_*number*
	Each person's keyring, sealed (see KEYRINGS).

_/system/state/authd/console/_*name*.toml
	Each account's second steps for the web console, and its API
	tokens (see WEB CONSOLE).

_/system/settings/admin.toml_
	The web console's sign-in rules, which authd reads (*admin.toml*(5)).

_/system/state/sign-ins_
	The sign-in records (see SIGN-IN RECORDS).

_/system/live/authd/boot_
	The time of the start of the computer that runs, as authd recorded
	it in the sign-in records.

_/system/live/authd/socket_
	The socket authd answers at.

_/system/live/authd/tmp/_*number*
	Each person's own _/tmp_, which only root lists.

_/users/_*name*
	Each account's home.

# EXIT STATUS

As a service, authd does not exit but when it is stopped, with 0; it
exits with 1 if it cannot start.

# SEE ALSO

*login*(1), *sshd*(8), *help-session*(1), *dukovo-rescue*(8), *elevate*(8), *sudo*(8), *user*(8), *passwd*(1), *keyring*(1), *id*(1), *last*(1),
*lastlog*(8), *consoled*(8), *admind*(8), *admin.toml*(5), *dukovo-admin*(7), *service*(8),
*timerd*(8), *crontab*(1), *service.toml*(5), *blocking.toml*(5), *firewall*(8), *spark*(8),
*logd*(8), *radix-names*(2), *radix-firewall*(2), *radix-posix*(7), *systools*(1)
