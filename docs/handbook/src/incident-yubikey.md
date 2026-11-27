<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# A lost or stolen YubiKey

Each of the project's two YubiKeys holds three of its keys: the system
releases' key in slot 95, the catalog's in slot 9c, and an age key that
opens the security team's key ([Keys and incidents](keys-and-incidents.md)).
Either YubiKey signs alone, so losing one stops nothing: the other signs
while you follow this page. A thief also needs its PIN, and three wrong
PINs lock it. Still, treat a lost YubiKey as stolen, and do every step
the same day: once the next roots are served, the lost one can sign
nothing a machine takes.

You need the source tree, on the computer that signs releases, the
YubiKey that is left, and the login that deploys to the update server.
A new YubiKey (a YubiKey 5 series key, as the project's two are) is
added in step 6, the same day if you have one at hand, or once it
arrives.

Below, `LOST` is the serial number of the YubiKey that is gone, `KEPT`
that of the one left, and `NEW` that of a new one. A YubiKey's serial
number is on its label, and `ykman list --serials` prints those plugged
in. The files `releases/keys/yubikey-SERIAL.json` name the YubiKeys the
roots name now.

## 1. Drop it from the system's releases

Take its files out of the source tree:

```
git rm releases/keys/yubikey-LOST.json releases/keys/yubikey-LOST.pem
```

If a new YubiKey is at hand, set it up first, as step 6 says, so that
one root drops the old and names the new. Then plug in the YubiKey that
is left, and the new one if there is one, and sign the next root:

```
./ob release root
```

It reads what updates.dukovo.org serves into `out/release-root/N`, `N`
being the next root's version, and shows that root before it signs.
Check what it shows:

- `every role:` names `KEPT`, and `NEW` marked `new` if you added it.
- One `dropped:` line, the lost YubiKey's key. The online key is not
  `new`.
- `signed on` names the YubiKeys plugged in.
- `then the targets again as they are, as version T`: `T` is one more
  than the targets version you last signed.

If all of that is so, type `root N`. Each YubiKey asks for its PIN and a
touch, then the YubiKey that is left signs the targets again under the
new root. It writes `releases/metadata/N.root.json`, and prints its
SHA-256 and the command that serves it:

```
./ob release deploy out/release-root/N
./ob release check
```

The deploy ends by reading the releases back as a computer does, which
now walks to root `N`. Commit `releases/keys/` and
`releases/metadata/N.root.json` on a branch, and open a pull request; the
deploy does not wait for it.

If it says the root names one YubiKey, that is so until step 6.

## 2. Drop it from the catalog

The same for the catalog's keys, in `catalog/` (`catalog/README.md`):

```
git rm catalog/keys/yubikey-LOST.json
./ob catalog root
./ob catalog sign --renew
./ob catalog deploy
./ob catalog pr
```

Add the new YubiKey's catalog key with `./ob catalog key yubikey
--serial NEW` before `./ob catalog root` if it is at hand. `catalog root`
signs the next root and nothing else, so `sign --renew` signs the targets
again as they are, with the YubiKey that is left: targets the lost one
signed last no longer check under the new root. `catalog pr` opens the
pull request that records what was deployed.

## 3. The install images' signature

If the images of the newest release are signed by the lost YubiKey
(`SHA256SUMS.yubikey-LOST.sig` beside them), sign their checksums again
with the one left, so that the signature checks with a key the tree
still holds. In the folder `./ob release images` made, from the top of
the source tree:

```
V=1.0.0-alpha.1
D=out/release-images/$V
rm $D/SHA256SUMS.yubikey-LOST.sig $D/SHA256SUMS.rekor.json
target/programs-host/debug/system-release sums \
    --root releases/metadata/N.root.json \
    --yubikey-helper tools/yubikey-piv.py --serial KEPT \
    --version $V --out $D --record $D/SHA256SUMS.rekor-entry.json \
    $D/dukovo-$V-x86_64.img $D/dukovo-$V-x86_64.iso
./ob release record $V
```

Publish `SHA256SUMS.yubikey-KEPT.sig` beside the download in place of
the old signature.

## 4. The security team's key

Security reports are sealed to the security team's key, whose private
half is kept wrapped to the YubiKeys' age keys (ADR 0531). A thief with
the lost YubiKey and its PIN could open it only with that wrapped file,
which stays on the maintainer's computer. Make a new team key anyway, so
that the next release seals reports to a key the lost YubiKey cannot
open:

```
age-keygen -o team.key
age-plugin-yubikey --list
age -r RECIPIENT -o team.key.age team.key
shred -u team.key
```

`age-keygen` prints the new key's public half, `age1…`. `age-plugin-yubikey
--list` prints the age recipient of each YubiKey plugged in; give `age`
one `-r` for each YubiKey that is left and each new one. Put the public
half in `userspace/report/security-recipients.txt` in place of the old
one, and commit it; the next release carries it. Keep the old wrapped
file until the reports sealed to the old key have been read, then
delete it.

## 5. If it was stolen

A stolen YubiKey and its PIN sign only where the thief can also serve
machines what they signed: from the update server, or from a place on
a machine's network that answers for updates.dukovo.org. Machines that
have taken root `N` refuse it.

- Check that the server serves only what you deployed: `./ob updates
  status`, `./ob release check` and `./ob catalog check`.
- If you think the PIN was known, or the server may have been reached,
  follow [A hacked update server or hosting account](incident-server.md)
  too.
- Report the theft where the YubiKey was kept, and say in the next
  release notes that the key was replaced.

## 6. A new YubiKey

Set up its PIV application as `catalog/README.md`'s "Setting up a
YubiKey" says, steps 1 to 5, and then add its keys:

```
ykman piv keys generate --algorithm ECCP256 --pin-policy ONCE --touch-policy ALWAYS 95 -
./ob release key yubikey --serial NEW
./ob catalog key yubikey --serial NEW
age-plugin-yubikey --generate
```

`ykman piv keys info 95` must fail before the first line, saying the slot
holds no key. Then sign the next roots as steps 1 and 2 do, with `KEPT`
and `NEW` plugged in, if they did not name it already, and wrap the team
key to it as step 4 does. Keep the two apart again: one at hand, the
other in its safe place.

If the lost YubiKey turns up, do not put it back as it was: reset its
PIV application, `ykman piv reset`, and set it up as a new one.
