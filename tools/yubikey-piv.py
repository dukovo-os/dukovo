#!/usr/bin/python3
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""Signs the catalog's metadata with a YubiKey's PIV key (ADR 0381).

    yubikey-piv.py list
    yubikey-piv.py public SERIAL
    yubikey-piv.py sign SERIAL < MESSAGE

catalog-repo(1) runs this, through `ob catalog`, on the computer the
YubiKeys are plugged into. `list` prints the serial number of each
YubiKey plugged in, one a line. `public` prints, in PEM, the public half
of the ECDSA P-256 key in the Digital Signature slot (9c) of the YubiKey
with that serial number. `sign` asks for its PIN on the terminal, asks
for a touch, and prints in hex the DER signature, ECDSA with SHA-256, of
what it reads; the secret half never leaves the YubiKey.

It uses Yubico's yubikit, from yubikey-manager (`sudo apt install
yubikey-manager` on Debian, Ubuntu and Linux Mint), with the system's
Python, which is why it names /usr/bin/python3. The key itself is made
with `ykman piv keys generate`, as catalog/README.md says.
"""

import getpass
import sys

try:
    from cryptography.hazmat.primitives import hashes, serialization
    from ykman.device import list_all_devices
    from yubikit.core.smartcard import SmartCardConnection
    from yubikit.piv import KEY_TYPE, SLOT, TOUCH_POLICY, InvalidPinError, PivSession
except ImportError as err:
    sys.exit(
        f"yubikey-piv: {err}: install yubikey-manager "
        "(sudo apt install yubikey-manager) for /usr/bin/python3"
    )


def device(serial):
    for dev, info in list_all_devices():
        if info.serial == serial:
            return dev
    sys.exit(f"yubikey-piv: no YubiKey with serial number {serial} is plugged in")


def public(serial):
    with device(serial).open_connection(SmartCardConnection) as conn:
        piv = PivSession(conn)
        metadata = piv.get_slot_metadata(SLOT.SIGNATURE)
        if metadata.key_type != KEY_TYPE.ECCP256:
            sys.exit(f"yubikey-piv: slot 9c of YubiKey {serial} holds no ECDSA P-256 key")
        if not metadata.generated:
            sys.exit(f"yubikey-piv: the key in slot 9c of YubiKey {serial} was imported, "
                     "not made on it, so a copy may exist; make one with ykman")
        if metadata.touch_policy != TOUCH_POLICY.ALWAYS:
            print(f"yubikey-piv: warning: the key of YubiKey {serial} signs without a touch",
                  file=sys.stderr)
        pem = metadata.public_key.public_bytes(
            serialization.Encoding.PEM, serialization.PublicFormat.SubjectPublicKeyInfo
        )
        sys.stdout.write(pem.decode())


def sign(serial):
    message = sys.stdin.buffer.read()
    with device(serial).open_connection(SmartCardConnection) as conn:
        piv = PivSession(conn)
        pin = getpass.getpass(f"PIN of YubiKey {serial}: ")
        try:
            piv.verify_pin(pin)
        except InvalidPinError as err:
            sys.exit(f"yubikey-piv: wrong PIN; {err.attempts_remaining} tries left")
        print(f"Touch YubiKey {serial} now.", file=sys.stderr, flush=True)
        signature = piv.sign(SLOT.SIGNATURE, KEY_TYPE.ECCP256, message, hashes.SHA256())
        print(signature.hex())


def main(args):
    if args == ["list"]:
        for _, info in list_all_devices():
            if info.serial is not None:
                print(info.serial)
    elif len(args) == 2 and args[0] in ("public", "sign") and args[1].isdigit():
        {"public": public, "sign": sign}[args[0]](int(args[1]))
    else:
        sys.exit("usage: yubikey-piv.py list | public SERIAL | sign SERIAL < MESSAGE")


if __name__ == "__main__":
    main(sys.argv[1:])
