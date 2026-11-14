<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Move the update server to Dukovo

updates.dukovo.org runs on an Ubuntu droplet today (Caddy, systemd, the
files of `tools/update-server/`). This page moves it to a droplet that
runs Dukovo, with nginx, certd, Spark's services and timers, so that the
project is the first user of its own update server (ADR 0542). It takes
about an hour of work, and one day of watching before the old droplet is
left alone, and a week before it is removed.

Three machines come up in the steps. Your **laptop** has the source tree
and the release and catalog keys. **OLD** is the Ubuntu droplet that
serves now. **NEW** is the droplet you make. Where a step says `OLD-IP`
or `NEW-IP`, use that droplet's address; `OLD-LOGIN` is the login you
deploy with to OLD today, which has `sudo` there.

Nothing on OLD is changed, so going back is always a change of the DNS
record (see [Going back](#going-back)). Do the steps in order. Each says
what to type, what you should see, and what to do if you see something
else. If a step does something this page does not say, stop there and tell
whoever is helping you: nothing is lost, as OLD still serves.

The online keys, the two keys that sign the timestamps for every machine,
go from OLD to NEW over ssh and never to your laptop. Do not copy them
anywhere else.

## Before the day

Lower the time to live of the `updates` A record to 300 seconds, where
dukovo.org's DNS is managed, one day before you start, so that the switch
in step 10 takes five minutes and not an hour. Check it from the laptop:

```console
$ dig updates.dukovo.org A +noall +answer
updates.dukovo.org.   300   IN   A   OLD-IP
```

If the number after the name is not 300, it has not taken effect yet:
wait until it has.

## 1. Build the image and make the droplet

On the laptop, in the source tree of the release you move to:

```console
$ ./ob image --cloud
```

It writes five disk files into `out/`; the one for DigitalOcean is
`out/dukovo-VERSION-x86_64-cloud.qcow2` (the handbook's *Cloud and
virtual machines* chapter lists the others). If it says `qemu-img` is
missing, install the `qemu-utils` package and run it again.

In DigitalOcean's control panel, *Backups & Snapshots*, *Custom Images*,
upload the `.qcow2` file, in the region OLD is in. Wait until the image
is listed as available. Make a droplet from it, with your SSH key, of at
least OLD's size, named `updates-dukovo-new`, and in the same project as
OLD. Write down its address: this is NEW-IP.

If the droplet has a cloud firewall, let ports 22, 80 and 443 in, as OLD's
firewall does.

If the upload says the file is not a disk image it takes, upload
`out/dukovo-VERSION-x86_64-cloud.img` instead, and tell the person
helping you.

## 2. Sign in to NEW

```console
$ ssh dukovo@NEW-IP
$ elevate service list --failed
```

The first command asks you to trust the host key: answer yes, and write
down the fingerprint it shows. The second prints nothing that says a
service failed. `dukovo` is the administrator that the image makes for the
SSH key you gave DigitalOcean; it elevates without a password.

If the sign-in is refused, wait a minute: the machine sets itself up at
its first start. If it is still refused after five minutes, look at the
droplet's console in the control panel.

Look at the machine's clock and the certificate it has of its own:

```console
$ date
$ elevate cert list
```

`date` is right to the minute, and `cert list` names `this-machine`. A
wrong clock stops certificates: fix that before going on.

## 3. Get the certificate for updates.dukovo.org

Until the DNS record points at NEW, Let's Encrypt cannot reach NEW on port
80, so this step proves the name with a record in DNS, which also lets you
check NEW over HTTPS before anyone is sent to it. From the laptop:

```console
$ ssh -t dukovo@NEW-IP elevate cert get updates.dukovo.org --dns --agree-terms
```

It prints a TXT record to add, as a line of a zone file, named
`_acme-challenge.updates.dukovo.org`, and waits. Add that record in the
DNS panel, with a short time to live, and press Enter. It looks the
record up every 10 seconds for up to 10 minutes, and then installs the
certificate and says when it runs out. It does not renew a certificate
proved this way: step 11 replaces it.

```console
$ ssh dukovo@NEW-IP elevate cert list
```

shows `updates.dukovo.org` with a day about 90 days ahead.

If the record is not found in 10 minutes, look at the zone: the name must
be exactly the one printed, and `dig TXT _acme-challenge.updates.dukovo.org`
from the laptop must show it. If the authority refuses the name, nothing
is installed: fix what it says and run the command again. Remove the
record again later; it does no harm there.

## 4. Bring the online keys from OLD

Sign in to OLD with your agent forwarded, so that OLD can sign in to NEW
as you, and the keys travel from OLD to NEW and not through the laptop:

```console
$ ssh -A OLD-LOGIN@OLD-IP
OLD$ NEW=dukovo@NEW-IP
OLD$ ssh "$NEW" 'umask 077 && mkdir /tmp/dukovo-update-keys'
```

OLD asks whether to trust NEW's host key: answer yes only if its
fingerprint is the one you wrote down in step 2. `/tmp` on NEW is in
memory: the keys never reach its disk.

```console
OLD$ sudo base64 -w0 /var/lib/dukovo-catalog/online.key | ssh "$NEW" 'umask 077 && cat > /tmp/dukovo-update-keys/catalog.key'
OLD$ sudo base64 -w0 /var/lib/dukovo-system/online.key | ssh "$NEW" 'umask 077 && cat > /tmp/dukovo-update-keys/system.key'
OLD$ ssh "$NEW" 'umask 077 && cat > /tmp/dukovo-update-keys/catalog.json' < /usr/local/share/dukovo-catalog/online.json
OLD$ ssh "$NEW" 'umask 077 && cat > /tmp/dukovo-update-keys/system.json' < /usr/local/share/dukovo-system/online.json
OLD$ ssh "$NEW" 'ls -l /tmp/dukovo-update-keys'
```

The last command lists four files, `catalog.json`, `catalog.key`,
`system.json` and `system.key`, none of them empty. Then leave OLD:

```console
OLD$ exit
```

If a command says permission denied for the `.key` files, you did not use
`sudo` for `base64`: the keys are read by their own users only. If `ssh`
on OLD says it has no key to offer, the agent was not forwarded: sign in
to OLD again with `ssh -A`. If a folder `/tmp/dukovo-update-keys` is
already there, a run was begun and stopped: remove it with
`ssh dukovo@NEW-IP rm -rf /tmp/dukovo-update-keys` and start this step
again.

## 5. Set NEW up

On the laptop:

```console
$ export OB_UPDATE_SSH=dukovo@NEW-IP
$ ./ob catalog server setup --dukovo
```

It builds `catalog-repo` for Dukovo, copies the server's files with `scp`
and runs `setup` on NEW through `elevate`. Everything it prints that
starts with `==>` is a stage. You should see, in this order, `The
folders`, `The program and the scripts`, `The service files`, `The online
keys` with the lines `Taking the catalog online key from
/tmp/dukovo-update-keys.` and `Taking the system online key from
/tmp/dukovo-update-keys.`, `nginx`, `The jobs` with six lines naming the
`dukovo-` jobs and their next times, then `nginx answers port 80 with
301`, and last `Done.` with the two places the keys' public halves are in.
There is no line saying `has no certificate yet`: step 3 gave it one.

Check that the public halves are the ones OLD has, and that the keys'
folder is gone:

```console
$ ssh dukovo@NEW-IP sha256sum /var/lib/dukovo-catalog/online.json /var/lib/dukovo-system/online.json
$ ssh OLD-LOGIN@OLD-IP sha256sum /usr/local/share/dukovo-catalog/online.json /usr/local/share/dukovo-system/online.json
$ ssh dukovo@NEW-IP ls /tmp/dukovo-update-keys
```

The two pairs of numbers are the same, and the last command says there is
no such folder.

If `setup` says the folder must belong to `dukovo` with the mode 700, step
4 was done as another login: remove the folder and do step 4 again. If it
says the online key is set and is never replaced, `setup` ran before: leave
the key alone and go on. If it ends with `nginx did not start`, run
`ssh dukovo@NEW-IP elevate log nginx` and read its last lines: it names the
setting that is wrong. If it says `has no certificate yet`, step 3 did not
finish: do it, then run the command of this step again, which is safe to
repeat.

## 6. Carry the version numbers over

Each machine that has read the old server remembers the version of the
timestamp and snapshot it saw, and refuses a lower one, as every TUF client
does. A new server that starts at 1 is refused by them until it has counted
past what they saw. If no machine but yours has read updates.dukovo.org yet,
skip to step 7. Otherwise, from OLD, give NEW the metadata folders of what
OLD serves, so that it goes on counting from there:

```console
$ ssh -A OLD-LOGIN@OLD-IP
OLD$ NEW=dukovo@NEW-IP
OLD$ sudo tar -C "$(readlink -f /srv/dukovo/system)" -cf - metadata | ssh "$NEW" 'cat > /tmp/system-metadata.tar'
OLD$ sudo tar -C "$(readlink -f /srv/dukovo/catalog)" -cf - metadata | ssh "$NEW" 'cat > /tmp/catalog-metadata.tar'
OLD$ exit
$ ssh dukovo@NEW-IP
$ elevate sh -c 'mkdir -p /var/lib/dukovo-system-site/system.seed && tar -xf /tmp/system-metadata.tar -C /var/lib/dukovo-system-site/system.seed && ln -sfn system.seed /var/lib/dukovo-system-site/system'
$ elevate sh -c 'mkdir -p /var/lib/dukovo-catalog-site/catalog.seed && tar -xf /tmp/catalog-metadata.tar -C /var/lib/dukovo-catalog-site/catalog.seed && ln -sfn catalog.seed /var/lib/dukovo-catalog-site/catalog'
```

Nothing in this step has been run before. The first deploy of step 7 takes
the seed as the folder served, signs the next numbers on top of it and
takes the seed away.

## 7. Deploy the newest release and catalog

Make the laptop ask NEW for updates.dukovo.org, so that what `ob` reads
back is NEW's, over HTTPS, with the real certificate. Add this line to
`/etc/hosts` on the laptop, and take it out again in step 10:

```
NEW-IP updates.dukovo.org
```

Then, with `OB_UPDATE_SSH` still set as in step 5, deploy as you always do:

```console
$ ./ob release deploy REPOSITORY
$ ./ob catalog deploy
```

`REPOSITORY` is the folder of signed releases you deploy today. The first
deploy sends every file, which takes a while; later ones send what NEW
does not have. Each prints `Copying the repository to dukovo@NEW-IP` and
waits for the server, up to 10 minutes: NEW's publish job looks once a
minute. Each ends by reading what the server serves, as a computer
online would, and prints the line of step 8.

If a deploy says the repository's first root is not
`releases/metadata/1.root.json`, the folder was not signed by the release
keys: give it the one that was.

If a deploy says the server did not publish, look at why:

```console
$ ssh dukovo@NEW-IP elevate service dukovo-system-publish status
$ ssh dukovo@NEW-IP elevate service list --timers
$ ssh dukovo@NEW-IP elevate log dukovo-system-publish
```

The status shows the job's last lines and when its timer starts it next;
the deploy's own message names what failed. A job missing from the timers
list means `setup` did not finish: run step 5 again. For the catalog, use
`dukovo-catalog-publish`. If the message says a file is not what its name
says, the deploy sent a damaged file: run the deploy again.

## 8. Check NEW as a client would

```console
$ ./ob release check
$ ./ob catalog check
```

The first prints `system-release: https://updates.dukovo.org/system/
offers N releases, newest Dukovo VERSION, signed from the trusted root`,
and a line for each channel. The second prints `https://updates.dukovo.org/catalog/:
the client reads it from the first root and fetches all N bundles.` N is
what you deployed, and VERSION is the newest release.

Check the answers' headers, that the numbers are not lower than OLD's, and
that the jobs have timers:

```console
$ curl -sI https://updates.dukovo.org/system/metadata/timestamp.json
$ curl -s https://updates.dukovo.org/system/metadata/timestamp.json | jq .signed.version
$ curl -s --resolve updates.dukovo.org:443:OLD-IP https://updates.dukovo.org/system/metadata/timestamp.json | jq .signed.version
$ ssh dukovo@NEW-IP elevate service list --timers
```

The first shows `HTTP/2 200`, `cache-control: no-cache` and
`strict-transport-security`. The second number, NEW's, is not lower than
the third, OLD's. The timers list has six lines named `dukovo-`, each with a
time next. Do the same for `/catalog/metadata/timestamp.json`.

If the numbers are lower and you skipped step 6, go back to it: do not
switch. NEW's numbers go up by one for every refresh, so it is faster to
seed it. If a check fails with a certificate error, the line in
`/etc/hosts` is missing or the certificate of step 3 is not there: run
`ssh dukovo@NEW-IP elevate cert list`. If `ob` reads OLD, not NEW,
`getent hosts updates.dukovo.org` on the laptop does not say NEW-IP: the
line in `/etc/hosts` is wrong.

## 9. Let the refresh jobs run once

The refresh jobs start 10 minutes after the machine starts, and every six
hours after that. Wait until each has run, then look:

```console
$ ssh dukovo@NEW-IP elevate service list --timers
$ ssh dukovo@NEW-IP elevate service dukovo-system-refresh status
$ ssh dukovo@NEW-IP elevate service dukovo-catalog-refresh status
```

Each of the two shows a last start after the first deploy, and no failure.
Do not go on before that: a server whose refresh does not work stops
serving updates in a day, when the timestamp it signed has expired.

## 10. Switch the DNS record

Take the line out of `/etc/hosts`. In the DNS panel, change the `updates`
A record from OLD-IP to NEW-IP, keeping the time to live at 300. Then:

```console
$ dig updates.dukovo.org A +noall +answer
$ ./ob release check
$ ./ob catalog check
```

`dig` shows NEW-IP within five minutes, and both checks print the lines of
step 8. From a machine that has never seen NEW, such as a phone off Wi-Fi,
`https://updates.dukovo.org/system/metadata/timestamp.json` opens without
a warning. If `dig` still shows OLD-IP after ten minutes, the record was
not saved or its time to live was not lowered a day before.

## 11. Replace the certificate with one certd renews

Now that the name leads to NEW, port 80 reaches nginx on NEW, which serves
the authority's answers. Get the certificate again, this time by HTTP:

```console
$ ssh -t dukovo@NEW-IP elevate cert get updates.dukovo.org --agree-terms --yes
$ ssh dukovo@NEW-IP elevate cert show updates.dukovo.org
```

The second command says `Proved by: http-01` and a day to renew by. If it
still says `dns-manual`, `cert get` took the way of the first one: certd
then does not renew it, and warns 21 days and 7 days before it runs out.
Run `cert get` with `--dns` again before then, and tell the person
helping you, so that it is fixed. If the first command fails, the
authority could not reach port 80: check the cloud firewall of step 1, and
that `dig` shows NEW-IP from outside your network. The old certificate
keeps serving until a new one is installed.

## Watching the first day

You are watching for three things: that the timestamps keep being signed,
that the daily jobs run, and that nginx is not failing. Look at these
four times, right after the switch, two hours later, in the evening and
the next morning:

```console
$ ssh dukovo@NEW-IP elevate service list --failed
$ ssh dukovo@NEW-IP elevate service list --timers
$ curl -s https://updates.dukovo.org/system/metadata/timestamp.json | jq .signed.version,.signed.expires
$ ssh dukovo@NEW-IP elevate log nginx
```

`--failed` lists none. The timers list has a time for each job; the
publish jobs show a last start within the last minute, the refresh jobs
within the last six hours, and the two daily jobs, which start between 03:00
and 04:30 by the machine's clock, a last start of that morning. The
timestamp's number goes up every six hours, and its `expires` is at least a
day ahead. The nginx log has a line for each request and no `[emerg]` or
`[crit]`. A refresh job that has not run for seven hours, or a timestamp
that expires in less than twelve, is a failure: go to [Going back](#going-back)
if it is not fixed in an hour.

Any deploy during the day goes to both servers, one after the other, with
`OB_UPDATE_SSH` set for each, so that going back loses nothing.

## Going back

OLD was not changed, and it serves as before. Point the A record back to
OLD-IP, with the time to live at 300; within five minutes `dig` shows it.
Then deploy to OLD whatever was deployed only to NEW:

```console
$ export OB_UPDATE_SSH=OLD-LOGIN@OLD-IP
$ ./ob release deploy REPOSITORY
$ ./ob catalog deploy
```

Machines that read NEW in the meantime remember its numbers, and refuse a
lower one from OLD. Compare the timestamp numbers of the two, as in step
8, and if OLD's is lower, start its refresh job, which raises it by one each
time, until it is not:

```console
$ ssh OLD-LOGIN@OLD-IP sudo systemctl start dukovo-system-refresh.service
$ ssh OLD-LOGIN@OLD-IP sudo systemctl start dukovo-catalog-refresh.service
```

If the snapshot's number, `jq .signed.meta` of the same file, is lower
on OLD, deploy again from the laptop until it is not: each deploy that
changes something raises it. If it does not rise, stop and tell the person
helping you; the machines that saw NEW's number will not update until it
does.

## After a week

If a week has passed with no failure, shut OLD down in the control panel,
and remove it after another week. Raise the time to live of the `updates`
record to what it was. The Ubuntu files in `tools/update-server/` (`setup.sh`,
the `.service`, `.path` and `.timer` units, the Caddyfile) are then
taken out of the tree in a later change.
