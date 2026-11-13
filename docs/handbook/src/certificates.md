<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Certificates

A service that serves TLS, such as the web console or nginx's HTTPS
server, needs a certificate that names the machine and the key that
goes with it. On Dukovo, one service, certd, keeps them, and Spark gives
each service a copy of the ones its service file names. The references
are cert(8), certd(8), service.toml(5) and dukovo-certificates(7); the
decisions are ADR 0512 and, for certificates from Let's Encrypt, ADR 0513.

| Task | Command |
| --- | --- |
| List the certificates, when each runs out and who uses it | `cert list` |
| Show one, with its fingerprint | `cert show NAME` |
| Print one's chain, for a client to trust | `cert show NAME --pem` |
| Get a certificate from Let's Encrypt, which certd then renews | `elevate cert get NAME --name DOMAIN` |
| Try that against Let's Encrypt's test authority first | `elevate cert get NAME --name DOMAIN --dry-run` |
| Get one for a wildcard, or with no port 80, proving the names by DNS | `elevate cert get NAME --name DOMAIN --name '*.DOMAIN' --dns` |
| Add a certificate from an authority, with its key | `elevate cert add NAME --chain FILE --key FILE` |
| Add its new version when it is renewed | `elevate cert add NAME --chain FILE --key FILE --replace` |
| Remove one | `elevate cert remove NAME` |
| Revoke one from Let's Encrypt | `elevate cert revoke NAME` |
| Renew one from Let's Encrypt now | `elevate cert renew NAME` |
| Make the machine's own again now | `elevate cert renew this-machine` |
| Check that the machine's own is there and has time | `cert check` |

Anyone may list and show certificates. Changing them needs an
administrator, so those commands go through elevate. A certificate's name
is 1 to 64 letters, digits, `-` and `.`, such as the site's name,
`example.com`.

## The machine's own certificate

certd makes a certificate called `this-machine` when it first starts. It
names the machine, the machine's name with `.local`, which
discoverd answers for on the local network (see
[Networking](networking.md)), and `localhost`. It is good for 800 days and certd makes it again 30 days
before it runs out, and whenever the machine is renamed.

Nobody but the machine vouches for it, so a browser warns of it, as it
does for any certificate you made yourself. Before you tell the browser
to trust it, check that the certificate it was shown is this machine's
and nobody's between: compare the fingerprints. The machine's:

```
$ cert show this-machine
Name:       this-machine
Kind:       self-signed
...
SHA-256:    3A:5F:...:C9
```

and the browser's, which it shows when you ask about the certificate it
warns of, are the same 32 pairs of digits. Then tell the browser to
trust it. Each time certd makes the certificate again its fingerprint
changes, and the browser asks again; the sign-in notice tells you so
the first time you sign in afterwards, with the new fingerprint.

A program on another machine can be given the certificate to trust in
place of a browser's question:

```
$ cert show this-machine --pem > this-machine.pem
$ curl --cacert this-machine.pem https://duke-nas.local/
```

`curl` then checks that the server is this machine, since the
certificate names it, and has no other authority.

## A certificate from Let's Encrypt

Let's Encrypt gives a certificate that every browser trusts, free, to
whoever proves that a name is theirs, and certd renews it before it runs
out, so that you do it once. `cert get` does the asking and the proving.
It needs three things:

1. **A name that leads to this machine.** The name, such as
   `example.com`, has an address record at your DNS provider (an `A`
   record, and an `AAAA` for IPv6) with the machine's public address.
   `cert get` looks the name up first and says when it does not resolve or
   leads to a private address, which Let's Encrypt cannot reach.
2. **Port 80 reaching the machine from the internet.** Let's Encrypt
   proves the name by fetching a small file from `http://example.com/.well-known/acme-challenge/`
   on port 80, whatever the site serves on 443. A machine behind a home
   or office router is not reached from outside until the router sends
   port 80 to it: in the router's settings, under *port forwarding* or
   *virtual server*, forward TCP port 80 from the router's outside address to the
   machine's address inside the network, and give the machine a fixed address (a
   *DHCP reservation* in the router) so that the forward keeps pointing at it. A provider that
   shares one public address among many homes (carrier-grade NAT) cannot
   forward ports: use the DNS way below.
3. **The firewall open to port 80.** Dukovo's firewall lets an
   administrator's command take connections only when a rule says so:

   ```
   $ elevate firewall allow tcp 80
   ```

   Where nginx holds port 80 already, it is nginx that answers, and this
   needs no rule (see below).

Try it against Let's Encrypt's test authority first, which has no
limits to run into and installs nothing:

```
$ elevate cert get example.com --name example.com --name www.example.com --dry-run
```

Its first run shows the authority's terms of service and asks whether
you agree: `--agree-terms` says so beforehand. `--email` gives a contact
address, which is optional: Let's Encrypt no longer mails notices of
expiry, and certd tells you itself. `cert get` checks the names and the firewall, and says what to fix
before it asks anything. When the dry run works, get the real one:

```
$ elevate cert get example.com --name example.com --name www.example.com
the names and this machine are ready
...
example.com installed: example.com, www.example.com, runs out 2027-02-12 (89 days)
issued by CN=R11
certd renews it before it runs out
```

The first name is the certificate's own; leaving out `--name` makes the
certificate's name the one name, as in `cert get example.com`. Name it in
a service's file as above and the service has it. certd looks at it
twice a day and renews it at a day Let's Encrypt suggests, about 30 days
before it runs out; `cert list` shows that day as `renews 2027-01-10`
once certd has chosen it. A renewal gives the services that use the
certificate its new files and has them read them, like any other change.
If a renewal fails, `log certd` says why, and from 14 days left the
sign-in notice and a notification say so too, critical from 7 days: then
`elevate cert renew example.com` tries again and shows the reason. A
rate limit is told with the time to try again.

`cert revoke example.com` revokes a certificate whose key you lost
track of; `--reason key-compromise` says why. The services keep using
it until you get another with `cert get example.com`, which replaces it.

### When nginx holds port 80

nginx's own server listens on port 80, so `cert get` cannot, and does not
need to: it puts the answer in a folder, `/system/live/certd/acme-challenge`,
and nginx serves that folder for every site that includes
`acme-challenge.conf`, as its default site does and a site you add should
(`include acme-challenge.conf;`). nginx has to be allowed to read the
folder, which its service file says once:

```
$ elevate service edit nginx          # add "/system/live/certd" to files.read
$ elevate service nginx restart
```

so that the line reads `files.read = ["/system/live/certd", ...]`. Then
`cert get` works as before, with no firewall rule: nginx is a service that
holds port 80, and the firewall lets it take connections unless a rule
says no. certd does the same when it renews. If another program holds
port 80, `cert get` says so, and the way is DNS.

### Proving the names by DNS

A wildcard certificate (`*.example.com`) can only be proved by DNS, and
so can a name on a machine that no router forwards port 80 to. With
`--dns` the authority never connects to the machine:

```
$ elevate cert get example.com --name example.com --name '*.example.com' --dns
Add these TXT records, then press Enter:
  _acme-challenge.example.com.  TXT  "9fK...x2c"
  _acme-challenge.example.com.  TXT  "Qp3...Zr8"
```

Add each as a `TXT` record at your DNS provider, both on the same name,
then press Enter. `cert get` asks the resolver for the records every 10
seconds, for up to 10 minutes, since DNS can be slow, and goes on once
they show. Remove the records afterwards if you like: they are of no use
again. certd cannot add DNS records, so it does not renew such a
certificate: it warns at 21 days and again at 7 days left, and you run
the same command again.

## A certificate from another authority

An authority, such as your organization's, sends the
certificate for a name, usually one file with the server's certificate
followed by the intermediate certificates (`fullchain.pem` or
`chain.pem`), and the key you made it for (`privkey.pem`). Add them under
the name of the site:

```
$ elevate cert add example.com --chain fullchain.pem --key privkey.pem
example.com added: example.com, www.example.com, runs out 2027-02-04 (89 days)
```

`cert add` checks the files before it keeps them, and says in words what
is wrong: the key is not the certificate's, the certificate has run out
or is not yet valid, it is an authority's and not a server's, it names no
machine, or the authorities Dukovo trusts do not vouch for it. The
check that people meet most is a chain that lacks its intermediate
certificate: Chrome fetches it by itself, and other browsers and programs
refuse the site. `cert add` says which one:

```
cert: the chain stops at CN=R10, which no authority Dukovo trusts signed: the file lacks
CN=R10's intermediate certificate, which your authority gives with the certificate (put it
after the certificate in chain.pem); a private authority that the clients trust and this
machine does not is taken with --untrusted
```

Put the intermediate certificate after the server's in the file and add
it again. For an authority of your own that your clients trust and this
machine does not, `--untrusted` takes the chain as it is; or trust the
authority on this machine too, as dukovo-certificates(7) says.

A certificate you added is never renewed by Dukovo: its authority renews
it, and you add its new version, which gives the services that use it the
new files and has them read them (an authority that speaks ACME is
reached with `cert get --directory`, and renewed by certd as Let's
Encrypt is):

```
$ elevate cert add example.com --chain fullchain.pem --key privkey.pem --replace
```

certd looks at the certificates you added twice a day. With 21 days or
fewer left, it puts a line in the sign-in notice and sends a notification
that says which certificate runs out and what to do; it is critical with
7 days or fewer left, and again once the certificate has run out.

## Giving a certificate to a service

A service names the certificates it uses in its file, in
`/system/settings/spark/services`:

```
certificates = ["this-machine"]
```

At each start, Spark copies the certificates into a folder that only the
service's account reads, and names the folder in the environment
variable `CERTIFICATES_DIRECTORY`. For each certificate there is a folder
of its name there with `fullchain.pem`, the certificate and its chain, and
`key.pem`. A service may name any certificate of `cert list`; Spark
refuses to start it when one does not exist.

When certd makes a certificate again, or `cert add --replace` swaps it,
Spark copies it again for each running service that names it and sends
the service SIGHUP, at most once in 10 seconds, and not in the first 10
seconds after the service started. A service that serves TLS reads its
certificates again on SIGHUP, without dropping a connection: nginx does.
The copy a service reads is always a whole old pair or a whole new one.

### nginx

nginx's HTTPS server is in `nginx.conf`, turned off, with the lines to
turn on. Name the certificate in nginx's service file, remove the `#`
signs before the second `server` block and restart nginx:

```
$ elevate service edit nginx          # add: certificates = ["this-machine"]
$ elevate nano /shared/services/org.nginx.nginx/nginx/conf/nginx.conf
$ elevate nginx -t
$ elevate service nginx restart
```

The block reads `/system/live/spark/certificates/nginx/this-machine/fullchain.pem`
and `key.pem` beside it. For a certificate you added, give its name
instead of `this-machine` in the service file and in the two lines (a
certificate from `cert get` is one). Later
renewals reach nginx without a restart, and requests being served are not
lost.

`cert list` names nginx in its USED BY column once the file does, and
`elevate cert remove` refuses to remove what a service file names,
naming the file, so that the service does not stop starting.

## Troubleshooting

`log certd` shows what certd did, a line for each certificate it made and
nothing when nothing was due. `cert list` and `cert show NAME` show what
is in the store.

| What you see | What it means and what to do |
| --- | --- |
| `cert: the clock is wrong (1970-01-01)` | The clock is before Dukovo's making: certd makes and judges no certificate until it is right, and looks again each minute. Set it (`timedatectl`) or let `timed` set it from the network. The service is ready when `cert check` passes, and Spark starts it again if that takes more than 2 minutes, which is harmless. |
| `cert: run it with elevate: changing certificates needs root` | `add`, `get`, `remove`, `renew` and `revoke` change the store: run them with `elevate`. |
| `the firewall closes port 80, which the authority must reach: open it with ...` | Let's Encrypt's check cannot reach the machine. Run `elevate firewall allow tcp 80`, as the message says, or prove the names by DNS with `--dns`. |
| `NAME does not resolve to an address` or `NAME leads to ADDRESS, a private address` | The name's DNS record is missing, or it leads to an address only your own network reaches. Give it the machine's public address, or use `--dns`. |
| `the authority could not reach http://NAME/.well-known/acme-challenge/` | The name leads somewhere, but port 80 does not reach this machine from the internet. Check the router's forward, and that the provider lets port 80 in. |
| `the authority reached NAME but got another answer` | Another server answers for the name: the address record leads to a different machine, or the router forwards port 80 elsewhere. |
| `port 80 is taken by another program: stop it, or use --dns` | A program other than nginx holds port 80. Stop it for the minutes `cert get` takes, or use `--dns`. |
| `the authority's rate limit was reached: try again after ...` | Let's Encrypt limits how many certificates a name gets in a week. Wait for the time it gives; try your steps with `--dry-run`, which has no such limit. |
| `the TXT record of NAME did not show within 10 minutes` | The record is not where the resolver looks: check the name it was added at and its value, wait for the provider to publish it, and run the command again. |
| `cert: certd or another cert is changing certificates; try again` | Another writer held the store for a minute. Run it again. |
| `certificates changed, but Spark did not copy them: ...; restart the services that use it` | The change is made; Spark did not take it up, for the reason given. `elevate service NAME restart` has the service read it as it starts. |
| `spark: could not start NAME: its certificate X does not exist ...` | The service's file names a certificate that is not in `cert list`. Add it, or take the name out of the file. |
| `spark: could not start NAME: certd's X ...` | Something in certd's folder is not as certd leaves it (a folder where a link should be, a file over 64 KiB, a link out of the store). Spark refuses what it cannot trust. `elevate cert renew this-machine` writes it again. |
| The browser warns that the connection is not private | For `this-machine`, expected: compare the fingerprints as above. For a certificate from an authority, `cert show NAME` says whether the system's authorities vouch for it. A certificate for a name the browser did not use, such as an address, warns too: `cert show NAME` lists the names. |
| The service still serves the old certificate | It does not read its certificates again on SIGHUP, as the services Dukovo ships do. `elevate service NAME restart` has it read them as it starts. |
| `cert remove`: `NAME names it in /system/settings/spark/services/NAME.toml: remove it there first` | A service file still names the certificate. Take the name out of the file and restart the service, then remove it. |
