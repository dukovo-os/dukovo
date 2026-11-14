service(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

service - start, stop and look at the services, and check their files

# SYNOPSIS

*service list* [*--failed*] [*--json*]

*service* _name_ *status* [*-n* _lines_]

*service* _name_ *explain* [*--json*]

*service* _name_ *start* | *stop* | *restart* [*--no-wait*] [*-q*]

*service* _name_ *reload* | *--full-restart*

*service* _name_ *logs* [_log options_]

*service* _name_ *enable* | *disable*

*service --status-all*

*service check* [*--brief*] [_name_ | _file_]...

*service new* _name_

*service edit* _name_

*service try* _name_ [*--for* _seconds_]

*service keep* _name_ | *undo* _name_

*service secret* *set* _name_ _secret_ | *list* _name_ | *remove* _name_ _secret_

# DESCRIPTION

service asks Spark, over its control socket _/system/live/spark/control_
(*spark*(8)), about the services it supervises: the programs that
_/system/settings/spark/services_ describes, a file each.

*list* shows every service, by name, with its state, whether it starts
at boot and since when it is in that state:

```
SERVICE   STATE                    AT BOOT  SINCE     DESCRIPTION
consoled  running                  on       10:41:02  Virtual consoles
lab-sick  running, health failing  off      10:44:51  Fails its check
lab-web   failed                   off      10:43:10  Lab web

Not running as asked: lab-sick and lab-web. `service lab-sick explain` says why.
```

The last line, on a terminal, names those not running as asked: given
up on, waiting to be started again after an end, stopped for a reason,
or failing their health check. One that is off as it is meant to be, a
driver the kernel left none of the cards it drives or a service Rescue
or essentials leave off, is not among them. *--failed* lists those alone and exits 1
when there are some, for scripts. *--json* prints Spark's *show*
document as it is (*spark*(8)), which has *show_version* for scripts to
check. In a pipe, the header is left out.

*status* shows what Spark knows of a service, what it may reach, its
processes and its last lines in the log:

```
nginx - nginx web server
   Active: active (running) as process 57 since 10:42:03, 2 min ago; ready in 10.4 s
    Needs: after netd, resolved
  Started: by boot
    Holds: its own view, acting for org.nginx.nginx
           reads /applications/org.nginx.nginx, /shared/www
           changes /system/state/org.nginx.nginx
           secrets tls-key
           listens on 80 (open), 443 (open); connects out
           memory up to 256M; CPU weight 100
Processes: 3 (57 nginx, 58 nginx, 59 nginx), 12.3M of memory, 1.2 s of CPU
  Restart: on-failure, 0 restarts so far

Nov 13 10:42:03 spark: started nginx (/applications/org.nginx.nginx/bin/nginx-service) as process 57
Nov 13 10:42:13 spark: nginx is ready (listening) 10.4 s after its start
```

and, for a service whose file names others, gives it a health check or
says when it gives up (*service.toml*(5), ADR 0470), why it is not
running, what it needs, what started it and how its checks go:

```
web - Web server
   Active: failed (Spark gave up on it) since 10:43:10, 5 min ago
      Why: it failed to start 5 times in a row (the last: it failed its health check 3 times in a row (exited with code 0x7))
    Needs: requires db; after netd
  Started: by a request
    Asked: to start by root (process 812) at 10:40:12, 8 min ago
   Health: failing, 3 checks in a row (exited with code 0x7)
  Restart: on-failure, 4 restarts so far; gives up after 5 failed starts in a row
     Last: exited with code 0x1 ("web: cannot reach db: connection refused")
```

*Active* is one of *active (running) as process* _n_, with how long it
took to be ready when its file says when it is; *active (exited)*, a
setup step that is done; *activating (starting as process* _n_*, ready
once* _when_*)*; *activating (waiting for* _names_*)*; *activating
(waiting to start again)*; *deactivating (stopping as process* _n_*)*;
*failed (Spark gave up on it)*; *inactive (disabled)* or *inactive
(dead)*; and since when, by the clock and how long ago. *Why* says why
it is not running as asked, *Needs* names the services its file names,
*Started* says whether boot, a request or another service wanted it,
*Asked* who last asked to start, stop or restart it, *Health* how its
last checks went, *Last* how its process last ended, with the last line
its output wrote before, and *File* what is wrong with its file while
Spark runs its last good copy.

*Holds* is what its file lets it reach beyond what every service reads
(*service.toml*(5)): its own view and the account it acts for, or *the
whole tree, with Spark's access* for one that runs with it; the places
it reads, changes and mounts beneath; the disks, or the EFI system
partitions, it reads, or reads and writes, as _/devices_ (*reads the
disks, as /devices*); the names at the top of the root it changes, and
nothing else there (*changes @snapshots at the top of the root, and
nothing else there*); *sees every process* when it is not the whole
tree and its file names _/processes_, where one that does not sees its
own processes alone (ADR 0473); its secrets; the ports it may
listen on, *(open)* for those something listens on now, and whether it
connects out, or *no network*; the machine's settings it may set, the
cards it drives; its memory limit and CPU weight. *Processes* counts the
processes in its domain, the first five by number and name, and their
memory and CPU time. The last lines are its records in the log, Spark's
lines about it among them, as *log -s* _name_ shows them (*log*(1));
*-n* _lines_ shows that many, 5 unless it is given, and *-n 0* none.
Status works when the log does not answer, without the lines.

*explain* says why a service is in its state, in one sentence, and what
to do about it. It follows the service it waits for, or the one that
stopped it, and the one that service waits for or was stopped by, up to
16 services, to the one at the root, and says what each is doing in
Spark's own words, as *Why* does (ADR 0471):

```
$ service lab-web explain
lab-web is stopped because it requires lab-broken, which was given up on: it failed to start 2 times in a row (the last: it exited with code 0x1) ("lab-broken: cannot open /shared/lab: no such folder"). Fix what stops lab-broken (`service lab-broken logs` shows what it wrote), then `service lab-web start` starts both.
```

The root is the last service followed, the one to look at. What to do
comes after the sentence when something can be done: fix the root and
start the service, start it again, add or repair the file of a
requirement, or take a service out of a circle; a service that runs
well, is starting, or waits for another that is on its way has nothing
to do. The sentence also says when the service's file does not read, and
that Spark runs its last good copy. The web console shows the same
sentence, since both make it from Spark's *show* document
(*spark*(8)). When a signature of *troubleshoot*(8) matches the root
(its log lines, and whether it failed), its fix replaces the generic one,
as "_fix_ (_ID_, `troubleshoot` says more)" (ADR 0474). *--json* prints it as one line of JSON with *sentence*;
*chain*, the names of the services followed, from the one asked about
to the root; *root*; and *fix*, what to do, or *null* when nothing
needs doing.

*start* starts a service that is stopped or disabled, with those it
requires and wants; *stop* stops one, with those that require it, and
keeps it stopped until it is started again or the system restarts; and
*restart* stops it and starts it again. Spark stops the domain of one
that has not ended 5 seconds after SIGTERM. Each waits until the service
has settled, and shows Spark's lines about it and the services the
request reaches as they come (ADR 0471):

```
$ service lab-web start
started nginx (/applications/org.nginx.nginx/bin/nginx-service) as process 57
nginx is ready (listening) 10.4 s after its start
started lab-web (/system/commands/lab-web) as process 61
```

```
$ service nginx stop
stopping lab-web: it requires nginx, which was stopped
nginx (process 57) was ended by signal 15
lab-web (process 61) was ended by signal 15
```

A start has settled once the service is ready, or is a setup step that
is done, and exits 0; or once a start of it, or of one it waits for,
has failed, was refused or was given up on, and exits 1 saying why:

```
$ service web start
service: web waits for db, which could not start: it exited with code 0x1 ("db: /shared/db: no such folder"); Spark tries db again in 0.4 s
```

A stop has settled once the service and those stopped with it have
ended. Each of these comes within the ready timeouts and the stop wait,
so a wait ends. Ctrl-C ends the wait and not the request, and exits 130:
*nginx goes on starting; `service nginx status` says how it goes*.
*--no-wait* (or *--no-block*) prints Spark's first answer and returns
at once, and *-q* prints only what went wrong. Four waits run at once;
a fifth start is answered at once, with *not watched: 4 watches
already*. *--full-restart* is *restart*, as Debian's.

*reload* sends a running service SIGHUP, which has consoled read its
keyboard layout's setting again (*consoled*(8)).

*logs* shows the service's records in the log, Spark's lines about it
among them, as *log -s* _name_ does, with *log*(1)'s options after it:
*-f* to follow, *-n* _lines_, *-b* for this boot, *--since* _time_,
*-p* _level_ and *-o* _format_.

*--status-all* lists every service, *[ + ]* for one that runs or is
done, *[ - ]* for one that does not, and *[ ? ]* for one starting,
waiting for others or waiting to be started again after it ended:

```
 [ + ]  consoled
```

The command line is Debian's.

*enable* turns a service on: it sets *enabled = true* in its file, so
that it starts at each boot, and starts it now, as
*systemctl enable --now* does; *disable* stops it and sets
*enabled = false*, so that it no longer starts at boot. Only root may.
A service whose file is not in _/system/settings/spark/services_ yet
gets a copy of the system's own there first, and the change is written
only if Spark still reads the file after it. This is how *sshd*(8) is
turned on:

```
$ sudo service sshd enable
sshd is running, and starts at each boot.
```

*check*, *new* and *edit* work on the services' files
(*service.toml*(5)), with the same reader Spark uses:

*check* [*--brief*] [_name_ | _file_]...
	Check the file of each service named, or each file given by a path
	(one with a */* or ending in _.toml_), or every file in
	_/system/settings/spark/services_. Prints _file_*: ok* for each that
	reads. For each that does not, it says where and what to do on the
	standard error as a compiler does: the file's path, the line and the
	column, what is wrong, the line with carets under the setting, and
	the fix:

	```
	/system/settings/spark/services/web.toml:6:1: `discription` is not a setting here; service.toml(5) lists them
	  |
	6 | discription = "Web"
	  | ^^^^^^^^^^^
	  = fix: did you mean `description`?
	```

	A mistake in an override file in _settings.d_ names that file and
	its line, and adds *= note: set over* the service's file. *--brief*
	prints the first line alone, with the fix after it, for a script or
	an editor's error list. A file is read with its override file in
	_settings.d_ over it, which *ok* names (*service.toml*(5)); it says
	which files are of before Phase 13 and not converted yet. When an
	update of a bundle kept a file an administrator changed and put the
	new one beside it as _name.toml.default_ (*bundled*(8)), it says so
	and which settings differ between the two.

	For a file that reads, *check* also says after *ok;* what putting it
	to use would do. It compares the file with the copy Spark read last,
	which only root reads, and asks Spark how its services stand. For a
	service that runs, it says which settings differ, what the restart
	that applies the change does and that the services that require it keep
	running:

	```
	/system/settings/spark/services/web.toml: ok; web runs an older copy: description differs; `service web restart` applies the change: it restarts web (its last start took 1.2 s); shop, which requires web, keeps running
	```

	For a service that does not run, it says when the settings apply, and
	if it would not start with them, why not:

	```
	/system/settings/spark/services/db.toml: ok; order differs from the copy Spark read last; it applies when db next starts
	```

	It says nothing after *ok* when nothing differs. For a file of a
	service Spark has no copy of, it says that Spark reads it when the
	service first starts. Without operands, *check* says it only of the
	files that differ, so a check of every file stays a line each. Run
	by one who is not root, *check* of a named file says *root sees what
	applying it does*. A file given by a path outside the services'
	folder, such as _./web.toml_, is checked as the file of the service
	its name gives, which says what putting it in place would do.

*new* _name_
	Write a new file for the service _name_ with every setting
	explained, which starts nothing at boot (*enabled = false*) and runs
	_/system/commands/true_ until it is edited. Refuses a name that is
	taken.

*edit* _name_
	Edit the service's file as *visudo*(8) edits sudoers: copy it to
	_.name.toml.edit_ beside it, open the copy in *$VISUAL*, *$EDITOR*
	or *nano*(1), and once the editor ends, check it as *check* does,
	with the file's override. A copy that does not read shows where and
	what is wrong, as *check* does, with the file's path rather than
	the copy's, and asks *What now? (e)dit again, or e(x)it without
	saving*. A copy that reads takes the file's place, and *edit* says
	*ok;* and what that does: the clause *check* gives, or *nothing to
	apply* when the service already runs those settings. The file
	changes only when its new text reads. Spark reads it when the
	service next starts: *service* _name_ *restart*.

*try* _name_ [*--for* _seconds_]
	Edit the service's file as *edit* does, checking the copy and saying
	what the change does before the copy takes the file's place, then
	try the change with an undo (ADR 0472), as *netplan try* tries a network's and Junos's
	*commit confirmed* a router's. Spark keeps the file as it was, the
	copy takes its place, and the service restarts, waiting as
	*restart* does. Then it asks *Keep the change to* _name_*?*: *y*
	keeps it, *n* undoes it now. Unless it is kept within _seconds_
	(60 unless *--for* says otherwise, 10 to 3600), from here or with
	*service keep* _name_ from another session, Spark puts the file
	back and restarts the service as it was. Spark undoes it at once
	when the service fails to start, or fails its health check, with
	the change, and before any service starts when the machine starts
	again first. So a change that cuts off the session that made it, as
	one to *sshd* can, comes undone by itself: a session that ends
	leaves the change to Spark, which undoes it when its time is up.
	Without a terminal to ask at, it returns once the service has
	restarted, for a script to keep the change once it has checked it.
	While it runs, *status* shows *Trying:* and the time left. A service
	Spark has not read since its file was written is not tried: start
	it first.

*keep* _name_
	Keep the change *service try* _name_ is trying.

*undo* _name_
	Undo it now: Spark puts the file back and restarts the service as it
	was.

*secret* keeps the secrets a service's file names, such as a database's
password, which Spark hands it as read-only files (*service.toml*(5),
ADR 0217). No command shows a value back.

*secret set* _name_ _secret_
	Set the service's secret: read its value from the terminal without
	showing it, or else from the standard input, less one line end
	after it, and keep it in
	_/system/state/spark/secrets/name/secret_, mode 0600. An empty
	value is refused. The service reads it when it next starts.

*secret list* _name_
	List the names of the service's secrets that are set.

*secret remove* _name_ _secret_
	Remove one.

# EXIT STATUS

0 for a request Spark took and a start, stop or restart that settled as
asked, for *status* of a service that runs or is done, and for
*explain* of one that is done or runs with its health check not
failing; 3 for *status* or *explain* of a service that does not run, or
is not ready yet, and for *explain* of one whose health check fails; 4
for a service Spark does not know; 1 when Spark refused the request,
could not be reached, or a start it waited for failed, and for
*list --failed* when a service is not running as asked; 130 when Ctrl-C
ended a wait; 2 for a command it does not know. *check* gives 1 when a
file does not read or an option is not one it knows, *new* when the file
is there already, *edit* when the edit was given up, *try* when the
change was undone, *keep* and *undo* when no change is being tried, and
*secret* when the secret could not be kept or removed.

# FILES

_/system/settings/spark/services/_*name*_.toml_
	The services' files (*service.toml*(5)).

_/system/live/spark/control_
	Spark's control socket.

_/system/state/spark/secrets/_*name*
	The service's secrets.

# SEE ALSO

*spark*(8), *service.toml*(5), *log*(1), *shutdown*(8), *sshd*(8), *systools*(1)
