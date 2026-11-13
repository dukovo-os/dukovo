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
decision is ADR 0512.

| Task | Command |
| --- | --- |
| List the certificates, when each runs out and who uses it | `cert list` |
| Show one, with its fingerprint | `cert show NAME` |
| Print one's chain, for a client to trust | `cert show NAME --pem` |
| Add a certificate from an authority, with its key | `elevate cert add NAME --chain FILE --key FILE` |
| Add its new version when it is renewed | `elevate cert add NAME --chain FILE --key FILE --replace` |
| Remove one | `elevate cert remove NAME` |
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

## A certificate from an authority

An authority, such as Let's Encrypt or your organization's, sends the
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
new files and has them read them:

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
instead of `this-machine` in the service file and in the two lines. Later
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
| `cert: run it with elevate: changing certificates needs root` | `add`, `remove` and `renew` change the store: run them with `elevate`. |
| `cert: certd or another cert is changing certificates; try again` | Another writer held the store for a minute. Run it again. |
| `certificates changed, but Spark did not copy them: ...; restart the services that use it` | The change is made; Spark did not take it up, for the reason given. `elevate service NAME restart` has the service read it as it starts. |
| `spark: could not start NAME: its certificate X does not exist ...` | The service's file names a certificate that is not in `cert list`. Add it, or take the name out of the file. |
| `spark: could not start NAME: certd's X ...` | Something in certd's folder is not as certd leaves it (a folder where a link should be, a file over 64 KiB, a link out of the store). Spark refuses what it cannot trust. `elevate cert renew this-machine` writes it again. |
| The browser warns that the connection is not private | For `this-machine`, expected: compare the fingerprints as above. For a certificate from an authority, `cert show NAME` says whether the system's authorities vouch for it. A certificate for a name the browser did not use, such as an address, warns too: `cert show NAME` lists the names. |
| The service still serves the old certificate | It does not read its certificates again on SIGHUP, as the services Dukovo ships do. `elevate service NAME restart` has it read them as it starts. |
| `cert remove`: `NAME names it in /system/settings/spark/services/NAME.toml: remove it there first` | A service file still names the certificate. Take the name out of the file and restart the service, then remove it. |
