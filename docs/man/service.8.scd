service(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

service - start, stop and look at the services, and check their files

# SYNOPSIS

*service list* [*--failed*] [*--json*]

*service* _name_ *status* [*-n* _lines_]

*service* _name_ *start* | *stop* | *restart* [*--no-wait*] [*-q*]

*service* _name_ *reload* | *--full-restart*

*service* _name_ *logs* [_log options_]

*service* _name_ *enable* | *disable*

*service --status-all*

*service check* [_name_ | _file_]...

*service new* _name_

*service edit* _name_

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

Not running as asked: lab-sick and lab-web. `service lab-sick status` says why.
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
it reads, changes and mounts beneath; its secrets; the ports it may
listen on, *(open)* for those something listens on now, and whether it
connects out, or *no network*; the machine's settings it may set, the
cards it drives; its memory limit and CPU weight. *Processes* counts the
processes in its domain, the first five by number and name, and their
memory and CPU time. The last lines are its records in the log, Spark's
lines about it among them, as *log -s* _name_ shows them (*log*(1));
*-n* _lines_ shows that many, 5 unless it is given, and *-n 0* none.
Status works when the log does not answer, without the lines.

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

*check* [_name_ | _file_]...
	Check the file of each service named, or each file given by a path
	(one with a */* or ending in _.toml_), or every file in
	_/system/settings/spark/services_. Prints _file_*: ok* for each that
	reads, and for each that does not, which line is wrong and why on
	the standard error; says which files are of before Phase 13 and
	not converted yet. A file is read with its override file in
	_settings.d_ over it, which *ok* names (*service.toml*(5)). When an
	update of a bundle kept a file an administrator changed and put the
	new one beside it as _name.toml.default_ (*bundled*(8)), it says so
	and which settings differ between the two.

*new* _name_
	Write a new file for the service _name_ with every setting
	explained, which starts nothing at boot (*enabled = false*) and runs
	_/system/commands/true_ until it is edited. Refuses a name that is
	taken.

*edit* _name_
	Edit the service's file as *visudo*(8) edits sudoers: copy it to
	_.name.toml.edit_ beside it, open the copy in *$VISUAL*, *$EDITOR*
	or *nano*(1), and once the editor ends, check it. A copy that reads
	takes the file's place; one that does not shows what is wrong and
	asks *What now? (e)dit again, or e(x)it without saving*. The file
	changes only when its new text reads. Spark reads it when the
	service next starts: *service* _name_ *restart*.

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
asked, and for *status* of a service that runs or is done; 3 for
*status* of a service that does not run, or is not ready yet; 4 for a
service Spark does not know; 1 when Spark refused the request, could
not be reached, or a start it waited for failed, and for
*list --failed* when a service is not running as asked; 130 when Ctrl-C
ended a wait; 2 for a command it does not know. *check* gives 1 when a
file does not read, *new* when the file is there already, *edit* when the edit
was given up, and *secret* when the secret could not be kept or removed.

# FILES

_/system/settings/spark/services/_*name*_.toml_
	The services' files (*service.toml*(5)).

_/system/live/spark/control_
	Spark's control socket.

_/system/state/spark/secrets/_*name*
	The service's secrets.

# SEE ALSO

*spark*(8), *service.toml*(5), *log*(1), *shutdown*(8), *sshd*(8), *systools*(1)
