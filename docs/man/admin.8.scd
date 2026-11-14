admin(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

admin - the web console from a terminal: its address, its actions and its API tokens

# SYNOPSIS

*admin fingerprint*

*admin actions* [*--json*]

*admin show* _action_ [_name_=_value_ | _name_:=_json_]...

*admin reach* [*private* | *any*]

*admin token add* _name_ *--scope* _scope_... [*--days* _n_]

*admin token list*

*admin token remove* _name_

*admin put-back* _id_

*admin export* _dir_

# DESCRIPTION

*admin* is the command that goes with the web console, Dukovo Admin
(*admind*(8), *dukovo-admin*(7), ADR 0515). It says where the console is
and which certificate it serves, lists what the console's API does, shows
what an action runs in the forms a person copies, says or sets whom the
console answers, makes, lists and removes the API tokens of the person who
types it, keeps the machine's settings in a Git repository of that
person's, and, as root, puts a change back.

An _action_ is one read or change that the console and its API offer, such
as *service.restart*. Each has an _id_ of its area and a verb. The
console runs an action's command, which is the command *show* prints: the
button, the API call and the command typed at a terminal do the same.

*actions*, *show* and *reach* with no word need no rights. *reach* with a
word is root's, as the file it changes is: *elevate*(8) it. *token* is the
person's own, asked of *authd*(8) as that person, so it can never make or
remove another's.

# COMMANDS

*fingerprint*
	Prints the console's address and the SHA-256 fingerprint of the
	certificate it serves, the two lines *admind*(8) writes in
	_/system/live/admind/issue_, which *consoled*(8) shows above the
	sign-in prompt. A person at a terminal or over SSH, with no screen to
	read them from, compares the fingerprint with the browser's warning
	the first time they open the console:

	```
	$ admin fingerprint
	https://duke-nas.local:9090
	SHA-256 fingerprint 3A:9F:0C:...:71:E2
	```

	When admind has not written the file, which is when it is not
	running, it says *The web console is not running: start it with
	elevate service admind start.*

*actions* [*--json*]
	Lists the actions, each with its id, whether it reads or changes,
	who runs its command, its method and address in the API, and after
	*e.g.* the command of its first example, here some of its lines:

	```
	$ admin actions
	ID                 KIND    RUNS AS        ROUTE
	service.list       read    the person     GET /api/1/services                    e.g. service list --json
	service.explain    read    the person     GET /api/1/services/{name}/explain     e.g. service diskhealth explain --json
	service.start      change  administrator  POST /api/1/services/{name}/start      e.g. elevate service diskhealth start
	service.restart    change  administrator  POST /api/1/services/{name}/restart    e.g. elevate service diskhealth restart
	certificate.show   read    the person     GET /api/1/certificates/{name}         e.g. cert show this-machine --json
	certificate.renew  change  administrator  POST /api/1/certificates/{name}/renew  e.g. elevate cert renew this-machine
	console.reach      change  administrator  POST /api/1/console/reach              e.g. elevate admin reach private
	```

	A read runs as the person, with the person's own rights, and always
	asks its command for JSON. A change runs as an administrator, through
	*authd*(8), as *elevate*(8) does, for an unlocked session of an
	administrator or a token whose owner is one.

	With *--json* it prints one JSON value, the list as *GET /api/1/actions*
	answers it: an array with an object for each action, with *id*,
	*title*, *area*, *changes*, *method*, *path*, *runs* (*as-the-person*
	or *as-administrator*), *long* (whether it runs as a task), *compares*
	(whether a change's *changed* is found by comparing what it moves
	before and after it, and false for a read), *errors* (the error IDs its
	command can end a line with), *tests* (the five tests that cover it),
	*input* (the JSON Schema of the arguments it takes) and *examples*.
	The area is also the scope a token needs: *services* for all of the
	area, *services:read* for reading it.

	*examples* is an object with *good*, a list of arguments that work on a
	machine as it is installed, which a test or a script can try, and
	*undo*: for a change that something puts back, an object with the
	*action* that does and the *args* it takes, so that a script that
	tries the change leaves the machine as it found it; for any other
	action, *null*. The examples of *service.stop* are:

	```
	{"good":[{"name":"diskhealth"}],"undo":{"action":"service.start","args":{"name":"diskhealth"}}}
	```

*show* _action_ [_name_=_value_ | _name_:=_json_]...
	Prints what *POST /api/1/show* answers for _action_ with the
	arguments given, and runs nothing. _name_=_value_ gives the argument
	_name_ the string _value_; _name_:=_json_ gives it the JSON value
	_json_, such as a number, *true* or a list. The arguments are checked
	as the action's own route checks them, and what cannot work is told in
	the words the console gives, with its error ID.

	The answer has four parts, each under its heading. *Command* is the
	command as one line, for Dukovo's shell and for sh, with *elevate* in
	front of what runs as an administrator. *API* is a *curl* line for the
	same action, with the token in *DUKOVO\_TOKEN*, against the console's
	address from _/system/live/admind/issue_, or *https://HOST:PORT* when
	there is none, for you to fill in. *Ansible* is the YAML of one task,
	for a playbook. *Files* lists the files a change writes, or *none*.

	```
	$ admin show service.restart name=diskhealth
	Command
	elevate service diskhealth restart

	API
	curl -sS -X POST -H "Authorization: Bearer $DUKOVO_TOKEN" -H "Content-Type: application/json" -d '{}' https://duke-nas.local:9090/api/1/services/diskhealth/restart

	Ansible
	- name: "Restart a service: diskhealth"
	  ansible.builtin.command:
	    argv:
	      - "service"
	      - "diskhealth"
	      - "restart"
	  become: true

	Files
	none
	```

*reach* [*private* | *any*]
	With no word, prints whom the console answers, *private* or *any*,
	from _/system/settings/admin.toml_ (*admin.toml*(5)): *private* is this
	machine, the networks of its cards and the address ranges no one
	reaches from the internet, and *any* is every machine. With a word,
	makes it what the word says. It changes *reach* in the *[console]*
	table of the file and nothing else, so that every comment in the
	file stays, and then has Spark reload *admind*(8) (as *service admind
	reload* does), which reads the file again at once. When the file
	says the word already it changes nothing and says so. A file that
	*admind* would refuse is not changed: *reach* says what is wrong with
	it. A word that is neither is a mistake in the command line. The
	change is root's, as the console's *console.reach* action runs it:

	```
	$ admin reach
	private
	$ elevate admin reach any
	The console's reach is now any: admind has read it again.
	```

	*any* asks for care: give administrators a second step first
	(*admin.toml*(5)). *admind* checks an address when it accepts a
	connection, so a connection that is made goes on, and the next one
	from an address that *private* does not answer is closed.

*token add* _name_ *--scope* _scope_... [*--days* _n_]
	Makes an API token named _name_ for the person who types it, which a
	script gives the console as *Authorization: Bearer* and the token
	(*dukovo-admin*(7)). It asks for the person's password, unseen at a
	terminal, or reads it as the first line of the standard input when
	there is no terminal. It prints the token on the standard output, once,
	and says on the standard error that it is not shown again: *authd*(8)
	keeps only its SHA-256.

	A _scope_ is *\**, all that the person may do; the name of an area, such
	as *services*, for reading and changing what the area holds; or the
	name with *:read* after it, for reading it alone. *--scope* may be
	given again, or take several scopes joined by commas, 16 at most. A
	token is never more than its owner. A change that runs as an
	administrator needs an owner who is an administrator and a scope that
	is *\** or names the area, not its reading alone. The scopes are
	checked before the password is asked.

	_name_ is 1 to 32 of *a* to *z*, *0* to *9* and *-*. The token lasts
	_n_ days, 1 to 366, 90 unless *--days* says. A person has 32 tokens at
	most. Only an account that may sign in to the web console makes one, and
	*authd*(8) says in its own words why it did not make one.

*token list*
	Lists the person's tokens: each one's name, its scopes and the last
	day it works, with *(ended)* for one whose day has passed, which
	*authd*(8) forgets 30 days later. It never prints a secret, which
	nothing keeps.

*token remove* _name_
	Removes the token _name_ at once. A script that still brings it is
	refused with ADM-115.

*put-back* _id_
	Writes back the earlier text of each file that change _id_ of the
	console's journal, "What changed", changed (ADR 0516). Only root runs
	it, which the console's *Put back* does, by *POST
	/api/1/changes/{id}/put-back* (*admind*(8)), as *elevate admin put-back*
	_id_, so that the command the console shows is the command that ran.
	It opens the console's store read-only, finds the change, and writes
	the earlier text of each of its files as *admind*(8) keeps it, keeping
	the file's owner and mode (a file that is made again is mode 0644). A
	file that holds the earlier text already is left alone and said so. A
	file that was changed again after the change is written over, and it
	says that the later text is gone.

	A service file, in _/system/settings/spark/services_, or its override
	in _settings.d_, is Spark's: Spark reads it when the service next
	starts, as it does for *service* _name_ *edit*. *put-back* writes it and
	says so, with the command that puts it to use now, *service* _name_
	*restart*, and asks Spark nothing, since a signal such as SIGHUP is for
	a service's own files and a service that does not handle it dies of it.
	A file in a bundle service's data folder, _/shared/services/ID/ACCOUNT_,
	is the service's own: each service that bundled's records name for that
	bundle and account is asked to read it again, as *service* _name_
	*reload* does. A service that does not run reads its file when it
	starts. When Spark cannot be asked, the files stay written, it says so,
	and it exits 1.

	A change cannot be put back if a file in it was made by it, which has
	no earlier text; is a secret or a file that not everyone may read,
	which the console never keeps; is one that *admind*(8) could not read;
	is longer than 1 MiB; or is one whose earlier text is among the
	versions the console has forgotten, as it keeps the last 20 of a file.
	Nor can a change that changed no file. It says which, and writes
	nothing.

	```
	$ elevate admin put-back 41
	put back /system/settings/spark/services/diskhealth.toml
	Spark reads diskhealth's service file when diskhealth next starts: `service diskhealth restart` puts it to use now
	change 41 is put back: 1 file
	```

*export* _dir_
	Copies the settings files that the console watches, the files under
	_/system/settings_, into _dir_, a Git repository, so that a person
	keeps the machine's settings history in their own Git (ADR 0516). Each
	file is put at the path it has on the machine, under
	_system/settings_ in _dir_. A _dir_ that is not there is made, and a
	new one or an empty one gets *git init*; a folder that has other files
	and is not a repository is refused, and nothing is written in it. A
	repository that has other files of the person's is changed in
	_system/settings_ and _SECRETS.md_ alone.

	Each export is one commit, "Settings of _name_ at _time_", with the
	machine's name and the time in UTC. It replaces what the last export
	put in _system/settings_, so a file that was removed from the machine
	is removed from the repository, and an export that changes nothing makes
	no commit and says so. The commit's author and committer are the
	machine's: the name "Dukovo on" _name_ and the address
	settings@_name_.invalid, which is no one's, whatever the person's own
	Git settings say. The person's hooks and signing do not apply to it.

	It runs as the person who types it, with their rights, and needs no
	elevate. A file that not everyone may read is a secret: it is left out
	by name, whatever the person's own rights, so that what everyone may
	not read is not in a repository that goes elsewhere. A file or folder
	that the person cannot read is left out too. _SECRETS.md_ in the
	repository lists every such path, as it is on the machine, and says
	why; the contents are never in the repository. It needs *git*.

	```
	$ admin export ~/duke-nas-settings
	37 files copied to /users/alice/duke-nas-settings
	2 secrets left out, since not everyone may read them: listed in SECRETS.md
	committed 3f2a1bc: Settings of duke-nas at 2026-11-14 08:00:00 UTC
	```

There is no *admin run*. The command that an action runs is the command
*show* prints, and a person runs it as it is, with *elevate*(8) where it
says so. A second way to run an action would be a second interface to
keep the same.

# FILES

_/system/live/admind/issue_
	The console's address and its certificate's fingerprint, which
	*admind*(8) writes when it starts and again when the certificate
	changes. It is not there while the console is not running.

_/system/settings/admin.toml_
	The console's settings, which *admin reach* reads and, as root,
	changes (*admin.toml*(5)).

_/system/state/authd/console/_*name*.toml
	The account's second steps and tokens, as hashes, which only root
	reads. *admin token* reaches them only through *authd*(8).

_/system/state/admind/store.db_
	The console's store, mode 0600, which holds the journal and the
	earlier versions of the settings files. *admin put-back* opens it
	read-only. It waits up to 5 seconds for a write of *admind*(8) to end.

# EXIT STATUS

0 when it did what was asked; 1 when it did not, saying why, which
includes a mistake in the command line, a refusal of *authd*(8), the
console not running for *fingerprint*, for *reach* a change that was not
made by root, a settings file that is not right and a Spark that did not
take the reload, a change that cannot be put back or a store that cannot
be opened for *put-back*, and a folder that *export* will not write in or
a *git* that does not run.

# EXAMPLES

Make a token that may read the services and change the certificates, for
a script that runs for a month:

```
$ admin token add monitor --scope services:read --scope certificates --days 30
Password:
dkv_alice_6c1f0a...
This token is not shown again: keep it now.
$ admin token list
NAME     SCOPES                       ENDS
monitor  services:read, certificates  2026-12-14
```

Ask the console for the services with it. The console's own certificate
is not one that *curl* knows: give it the certificate, which *cert show
this-machine --pem* prints, after checking its fingerprint against
*admin fingerprint*:

```
$ cert show this-machine --pem > console.pem
$ DUKOVO_TOKEN=dkv_alice_6c1f0a...
$ curl -sS --cacert console.pem -H "Authorization: Bearer $DUKOVO_TOKEN" \
    https://duke-nas.local:9090/api/1/services
```

Write the Ansible task that restarts a service, for a playbook:

```
$ admin show service.restart name=diskhealth
```

Open the console to every machine, and close it again:

```
$ elevate admin reach any
The console's reach is now any: admind has read it again.
$ elevate admin reach private
The console's reach is now private: admind has read it again.
```

Put back a change that someone made by hand to a service's file, which
the console's journal, "What changed", lists as change 41:

```
$ elevate admin put-back 41
```

Keep the machine's settings in a Git repository of your own, and again
after the next change:

```
$ admin export ~/duke-nas-settings
$ git -C ~/duke-nas-settings log --oneline
```

Remove the token when the script no longer runs:

```
$ admin token remove monitor
The API token monitor is removed.
```

# SEE ALSO

*admind*(8), *admin.toml*(5), *authd*(8), *dukovo-admin*(7), *elevate*(8),
*service*(8), *cert*(8), *passwd*(1), *git*(1)
