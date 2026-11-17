#!/usr/bin/python3
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""Signs TUF metadata with a YubiKey's PIV key (ADRs 0381 and 0538).

    yubikey-piv.py list
    yubikey-piv.py public SERIAL [SLOT]
    yubikey-piv.py sign SERIAL [SLOT] < MESSAGE

catalog-repo(1) and system-release(8) run this, through `ob catalog` and
`ob release`, on the computer the YubiKeys are plugged into. `list`
prints the serial number of each YubiKey plugged in, one a line.
`public` prints, in PEM, the public half of the ECDSA P-256 key in SLOT
of the YubiKey with that serial number: 9c, the Digital Signature slot,
which holds the catalog's key and is the default, or 95, which holds the
system releases' key. `sign` asks for its PIN, asks for a touch, and
prints in hex the DER signature, ECDSA with SHA-256, of what it reads;
the secret half never leaves the YubiKey.

`sign` asks for the PIN on the terminal, or through pinentry, the
desktop's own PIN window, when DUKOVO_PINENTRY is set or no terminal is
attached: Dukovo Ops sets it, so that a PIN never passes through the app.
DUKOVO_PINENTRY names the pinentry program; empty or `1` means
`pinentry`, which Linux Mint links to its desktop's (pinentry-gnome3 or
pinentry-gtk2).

It uses Yubico's yubikit, from yubikey-manager (`sudo apt install
yubikey-manager` on Debian, Ubuntu and Linux Mint), with the system's
Python, which is why it names /usr/bin/python3. The key itself is made
with `ykman piv keys generate`, as catalog/README.md says.
"""

import getpass
import os
import re
import subprocess
import sys
from types import SimpleNamespace

# A YubiKey's PIV PIN is 6 to 8 characters; one of another length is
# refused here, before it costs one of the key's tries.
PIN_LENGTHS = range(6, 9)

# How long pinentry waits for the PIN before it gives up, in seconds.
PINENTRY_TIMEOUT = 300

# Assuan, pinentry's protocol, takes lines of at most 1000 bytes.
ASSUAN_LINE_MAX = 1000

# How many times the PIN window asks before signing gives up.
PIN_ASKS = 5

# The slots a key may be in: the catalog's and the system releases'. A
# signature from one slot is never taken for the other repository's.
SLOTS = {"9c": 0x9C, "95": 0x95}


def yubikit():
    """Yubico's modules, imported only when a YubiKey is used."""
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
    return SimpleNamespace(
        hashes=hashes, serialization=serialization, list_all_devices=list_all_devices,
        SmartCardConnection=SmartCardConnection, KEY_TYPE=KEY_TYPE, SLOT=SLOT,
        TOUCH_POLICY=TOUCH_POLICY, InvalidPinError=InvalidPinError, PivSession=PivSession,
    )


def device(yk, serial):
    for dev, info in yk.list_all_devices():
        if info.serial == serial:
            return dev
    sys.exit(f"yubikey-piv: no YubiKey with serial number {serial} is plugged in")


def public(serial, slot):
    yk = yubikit()
    with device(yk, serial).open_connection(yk.SmartCardConnection) as conn:
        piv = yk.PivSession(conn)
        try:
            metadata = piv.get_slot_metadata(yk.SLOT(SLOTS[slot]))
        except Exception:
            # yubikit raises ApduError, whose module differs between its
            # versions, when the slot is empty.
            sys.exit(f"yubikey-piv: slot {slot} of YubiKey {serial} holds no key; "
                     "make one with ykman")
        if metadata.key_type != yk.KEY_TYPE.ECCP256:
            sys.exit(f"yubikey-piv: slot {slot} of YubiKey {serial} holds no ECDSA P-256 key")
        if not metadata.generated:
            sys.exit(f"yubikey-piv: the key in slot {slot} of YubiKey {serial} was imported, "
                     "not made on it, so a copy may exist; make one with ykman")
        if metadata.touch_policy != yk.TOUCH_POLICY.ALWAYS:
            print(f"yubikey-piv: warning: the key in slot {slot} of YubiKey {serial} signs "
                  "without a touch", file=sys.stderr)
        pem = metadata.public_key.public_bytes(
            yk.serialization.Encoding.PEM, yk.serialization.PublicFormat.SubjectPublicKeyInfo
        )
        sys.stdout.write(pem.decode())


def assuan_escape(text):
    """`text` as an Assuan command's parameter: %, CR, LF and every other
    control character percent-escaped."""
    return "".join(f"%{ord(c):02X}" if c == "%" or ord(c) < 0x20 else c for c in text)


def assuan_unescape(data):
    """An Assuan data line's bytes, its %XX escapes undone."""
    return re.sub(rb"%([0-9A-Fa-f]{2})", lambda m: bytes([int(m.group(1), 16)]), data)


class Cancelled(Exception):
    """The person closed the PIN window, or it timed out."""


class Pinentry:
    """One pinentry program, spoken to in Assuan over its pipes."""

    def __init__(self, program):
        try:
            self.process = subprocess.Popen(
                [program], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
            )
        except OSError as err:
            sys.exit(f"yubikey-piv: cannot start {program} to ask for the PIN: {err}; "
                     "install pinentry-gnome3 (sudo apt install pinentry-gnome3)")
        self.answer()

    def answer(self):
        """The data lines up to pinentry's OK, joined; raises Cancelled on
        its ERR."""
        data = b""
        while True:
            line = self.process.stdout.readline()
            if not line:
                sys.exit("yubikey-piv: pinentry ended without answering")
            line = line.rstrip(b"\r\n")
            if line == b"OK" or line.startswith(b"OK "):
                return data
            if line.startswith(b"D "):
                data += assuan_unescape(line[2:])
            elif line.startswith(b"ERR "):
                raise Cancelled(line[4:].decode(errors="replace"))
            # Comments (#) and status lines (S) say nothing needed here.

    def command(self, *words):
        line = " ".join(words).encode() + b"\n"
        if len(line) > ASSUAN_LINE_MAX:
            sys.exit("yubikey-piv: a line for pinentry is too long")
        self.process.stdin.write(line)
        self.process.stdin.flush()
        return self.answer()

    def close(self):
        try:
            self.process.stdin.write(b"BYE\n")
            self.process.stdin.close()
        except OSError:
            pass
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        self.process.stdout.close()


def pinentry_program():
    """The pinentry program to ask with, or None to ask on the terminal."""
    named = os.environ.get("DUKOVO_PINENTRY")
    if named is None:
        try:
            with open("/dev/tty"):
                return None
        except OSError:
            named = ""
    return "pinentry" if named in ("", "1") else named


def pin_by_pinentry(program, serial, error=None):
    """Asks for the PIN of YubiKey `serial` in the PIN window, with
    `error` above it when the last answer was wrong."""
    entry = Pinentry(program)
    try:
        entry.command("SETTITLE", assuan_escape("Dukovo: sign with a YubiKey"))
        entry.command("SETDESC", assuan_escape(
            f"Type the PIN of YubiKey {serial} to sign with it.\n"
            "The key asks for a touch next."))
        entry.command("SETPROMPT", "PIN:")
        entry.command("SETTIMEOUT", str(PINENTRY_TIMEOUT))
        if error:
            entry.command("SETERROR", assuan_escape(error))
        pin = entry.command("GETPIN")
    except Cancelled:
        sys.exit("yubikey-piv: no PIN was given: the PIN window was closed or timed out")
    finally:
        entry.close()
    try:
        return pin.decode()
    except UnicodeDecodeError:
        return ""


def sign(serial, slot):
    message = sys.stdin.buffer.read()
    yk = yubikit()
    program = pinentry_program()
    with device(yk, serial).open_connection(yk.SmartCardConnection) as conn:
        piv = yk.PivSession(conn)
        error = None
        for _ in range(PIN_ASKS):
            if program is None:
                pin = getpass.getpass(f"PIN of YubiKey {serial}: ")
            else:
                pin = pin_by_pinentry(program, serial, error)
            if len(pin) not in PIN_LENGTHS or not pin.isascii():
                if program is None:
                    sys.exit("yubikey-piv: a YubiKey's PIN has 6 to 8 characters")
                error = "A YubiKey's PIN has 6 to 8 characters."
                continue
            try:
                piv.verify_pin(pin)
                break
            except yk.InvalidPinError as err:
                left = err.attempts_remaining
                if program is None or left == 0:
                    sys.exit(f"yubikey-piv: wrong PIN; {left} tries left")
                error = f"Wrong PIN: {left} {'try' if left == 1 else 'tries'} left."
        else:
            sys.exit(f"yubikey-piv: no PIN was accepted after {PIN_ASKS} asks")
        del pin
        print(f"Touch YubiKey {serial} now.", file=sys.stderr, flush=True)
        signature = piv.sign(
            yk.SLOT(SLOTS[slot]), yk.KEY_TYPE.ECCP256, message, yk.hashes.SHA256()
        )
        print(signature.hex())


def main(args):
    if args == ["list"]:
        for _, info in yubikit().list_all_devices():
            if info.serial is not None:
                print(info.serial)
    elif len(args) in (2, 3) and args[0] in ("public", "sign") and args[1].isdigit():
        slot = args[2] if len(args) == 3 else "9c"
        if slot not in SLOTS:
            sys.exit(f"yubikey-piv: no slot {slot}: the slots are {', '.join(SLOTS)}")
        {"public": public, "sign": sign}[args[0]](int(args[1]), slot)
    else:
        sys.exit("usage: yubikey-piv.py list | public SERIAL [SLOT] | sign SERIAL [SLOT] < MESSAGE")


if __name__ == "__main__":
    main(sys.argv[1:])
