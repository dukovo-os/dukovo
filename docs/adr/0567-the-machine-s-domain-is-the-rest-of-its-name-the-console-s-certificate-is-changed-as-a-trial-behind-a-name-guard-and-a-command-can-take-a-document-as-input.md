<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0567. The machine's domain is the rest of its name, the console's certificate is changed as a trial behind a name guard, and a command can take a document as input

- Status: Accepted
- Date: 2026-11-16

## Context

The web console's Certificates page comes with C6. On it an administrator
gets a certificate from an authority, uploads one made elsewhere, gives
the machine its name and domain, and chooses the certificate the console
serves.

Three things stood in the way.

A machine's name was one word. certd made the machine's own certificate,
`this-machine`, for the name, the name with `.local` (which discoverd
answers for on the local network) and `localhost`. A name with dots in
it, such as `duke-nas.example.org`, was taken whole, so the certificate
named `duke-nas.example.org.local`, which nothing answers for, and not
`duke-nas.local`, which discoverd does. admind answered only to the names
of the certificate it served (ADM-114), so a browser on the local network
could not reach a machine named in a domain at all.

The console served the certificate `certificate` in admin.toml(5) names,
and only one Spark had copied for it: a service's file names its
certificates, and Spark copied them when the service started. Choosing
another certificate meant two files and a restart of admind, which cuts
off the page that asked before it is answered. And a certificate that
does not name the address a browser uses has that browser warn on every
new connection: an administrator who chose one from the machine's
address would lock their own browser behind a warning.

A secret reached a command on its input, through a pipe (ADR 0561), and a
pipe holds 4 KiB before its writer waits for the reader. A certificate's
key and chain are larger, and a page that uploads them could not send
them that way.

## Decision

**The domain is the rest of the name.** A name of several words, such as
`duke-nas.example.org`, is the first word, `duke-nas`, in the domain
`example.org`. this-machine names the full name, the first word, the
first word with `.local`, and `localhost`, in that order, with no name
twice; a one-word name is as before. certd makes again a certificate
that does not hold all of these, so a machine named before this change
gets them at its next start. admind answers to all of them, since its
names come from the same rule. `hostnamectl status --json` says the name,
the first word, the domain and the local name; `hostnamectl
set-hostname NAME --apply` sets it and then has discoverd, certd and
admind take it, and waits for the new certificate, so the console's
rename moves the browser to the new address rather than trying it.

**The console's certificate is one command, tried as the console's
settings are.** `admin certificate NAME [--try SECONDS]` changes
`certificate` in admin.toml and `certificates` in admind's service file
(or its override, when that sets them) together: the name is added,
`this-machine` stays, and the one served before goes. Spark's reload of a
service now copies the certificates its file names before it sends
SIGHUP, when they are not those the service started with; the rest of a
changed file still waits for a restart. So admind reads both files again
on its reload and serves the new certificate to the next connection, and
no restart cuts off the page. The command waits for admind's issue file
to name the new certificate's fingerprint, which admind writes once it
serves it. With `--try` both files are the trial `console` (ADR 0562),
which Spark puts back, reloading admind, unless `admin keep` comes in
time; a certificate that admind does not serve in time is undone at once.

**The guard is by name.** `CutsOff` (ADR 0562) guarded a change by the
network card a request came through. It now says which requests it
guards: those of one card, as before, or any request, by the name its
browser used for the console (the request's `Host`, without the port).
`console.certificate` is the first of the second kind: admind reads `cert
show NAME --json` as the caller's administrator and refuses (ADM-141)
unless the address is one of the certificate's names, as a browser checks
them: the same address for an IP address, the same name in small or
capital letters, and a wildcard for the left-most word alone. A read that
fails refuses, so the guard fails closed. The Certificates page offers
the change only for a certificate that names the address in use, and says
where to open the console for one that names the machine otherwise.

**A command can take a document.** An action's field can be a document
(`Field::document`), text of up to 64 KiB with no NUL, or a secret one
(`Field::secret_document`), and an action says how much input its command
may take (`Action::INPUT_MAX`, 4 KiB unless it says more, 128 KiB at the
most). admind writes input beyond what a pipe holds from a task of its
own while the command runs, and the command is answered only once that
writer has ended; a command that stops reading ends the writer, and a
secret's copy is wiped when it ends, as ADR 0561's is. The Show command's
API call takes a document from a variable that holds it as JSON text,
since a key's line ends cannot be typed inside a JSON string.

## Consequences

- A machine named in a domain is reached at `NAME.local` on its network
  and at its full name wherever the domain's records lead, with the same
  certificate.
- The console's certificate changes without a restart and is put back by
  itself when the page that changed it does not come back on it.
- `service NAME reload` does a little more than send SIGHUP: when the
  service's file names other certificates, they are copied first. A copy
  that fails is put back and the reload is refused, so the service runs
  with the certificates it had.
- A third kind of guard needs no new route code: an action names the
  requests it guards and how to read what the change would leave.
- The console's trial banner and ADM-138 say "the console's address or
  certificate", since one trial holds the reach and the certificate.
- The registry's tests feed documents of the largest size each action
  takes, and hold that a secret document is in nothing admind keeps.

## Alternatives considered

- **Restart admind under the trial.** Spark already restarts a tried
  service's file; but the restart ends the connection that asked, so the
  page never learns whether the change began, and the task's row would
  end as "the console restarted".
- **Name every certificate in admind's service file.** No change of the
  service file would be needed, but admind would hold the keys of every
  service's certificates, the mail server's and the websites' among them.
- **Copy certificates at every reload, whatever the file says.** It would
  copy unchanged certificates and send nothing new; the copy at a change
  of certd's (ADR 0512) already covers a certificate made anew.
- **A guard by the browser's address alone.** A browser at the machine's
  address and one at its name are the same machine to admind, but not to
  the browser, which checks the name it typed against the certificate.
- **A temporary file for a large input.** It would be written to a disk,
  which keeps it after it is removed, as ADR 0561 says of a secret.
