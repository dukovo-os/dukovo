#!/usr/bin/python3
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""Tests of yubikey-piv.py's PIN window and signing steps, with a
scripted pinentry and a stand-in for the YubiKey: `ob test unit` runs
them, and they need neither a YubiKey nor yubikey-manager."""

import importlib.util
import io
import os
import pathlib
import sys
import tempfile
import types
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("yubikey_piv", HERE / "yubikey-piv.py")
piv = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(piv)

# A pinentry that answers as pinentry-gnome3 does, keeps every command it
# was sent, and gives the PINs in its list one a run, or cancels.
FAKE_PINENTRY = """\
import os, sys
log = open(os.environ["FAKE_LOG"], "a")
answers = os.environ["FAKE_PINS"].split(",")
run = sum(1 for line in open(os.environ["FAKE_LOG"]) if line.startswith("GETPIN"))
print("OK Pleased to meet you", flush=True)
for line in sys.stdin:
    line = line.rstrip("\\n")
    log.write(line + "\\n")
    log.flush()
    if line == "GETPIN":
        answer = answers[min(run, len(answers) - 1)]
        if answer == "CANCEL":
            print("ERR 83886179 Operation cancelled <Pinentry>", flush=True)
            continue
        print("# a comment", flush=True)
        print("S PROGRESS 1", flush=True)
        print("D " + answer, flush=True)
    if line == "BYE":
        print("OK closing connection", flush=True)
        break
    print("OK", flush=True)
"""


class InvalidPinError(Exception):
    def __init__(self, attempts_remaining):
        super().__init__("wrong PIN")
        self.attempts_remaining = attempts_remaining


class FakeYubiKey:
    """A YubiKey whose PIN is 123456, with three tries."""

    def __init__(self):
        self.tries = 3
        self.pins = []

    def namespace(self):
        key = self

        class Session:
            def __init__(self, conn):
                pass

            def verify_pin(self, pin):
                key.pins.append(pin)
                if pin != "123456":
                    key.tries -= 1
                    raise InvalidPinError(key.tries)
                key.tries = 3

            def sign(self, slot, key_type, message, hash_):
                return b"\x30\x01" + message[:1]

        class Device:
            def open_connection(self, kind):
                return mock.MagicMock(__enter__=lambda s: s, __exit__=lambda s, *a: False)

        return types.SimpleNamespace(
            list_all_devices=lambda: [(Device(), types.SimpleNamespace(serial=7))],
            SmartCardConnection=object, PivSession=Session, InvalidPinError=InvalidPinError,
            SLOT=types.SimpleNamespace(SIGNATURE="9c"),
            KEY_TYPE=types.SimpleNamespace(ECCP256="p256"),
            hashes=types.SimpleNamespace(SHA256=lambda: None),
        )


class PinentryTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        folder = pathlib.Path(self.dir.name)
        self.program = folder / "pinentry"
        self.program.write_text(f"#!{sys.executable}\n" + FAKE_PINENTRY)
        self.program.chmod(0o755)
        self.log = folder / "log"
        self.log.write_text("")

    def environment(self, pins):
        return mock.patch.dict(os.environ, {
            "FAKE_LOG": str(self.log), "FAKE_PINS": ",".join(pins),
            "DUKOVO_PINENTRY": str(self.program),
        })

    def sent(self):
        return self.log.read_text().splitlines()

    def test_escaping_round_trips(self):
        text = "100% sure\nline two\r"
        escaped = piv.assuan_escape(text)
        self.assertEqual(escaped, "100%25 sure%0Aline two%0D")
        self.assertEqual(piv.assuan_unescape(escaped.encode()).decode(), text)

    def test_the_window_gives_the_pin_unescaped(self):
        with self.environment(["12%2534a6"]):
            pin = piv.pin_by_pinentry(piv.pinentry_program(), 7)
        self.assertEqual(pin, "12%34a6")
        sent = self.sent()
        self.assertIn("SETPROMPT PIN:", sent)
        self.assertTrue(any(line.startswith("SETDESC ") and "%0A" in line for line in sent))
        self.assertFalse(any(line.startswith("SETERROR") for line in sent))
        self.assertEqual(sent[-2:], ["GETPIN", "BYE"])

    def test_a_closed_window_stops_signing(self):
        with self.environment(["CANCEL"]), self.assertRaises(SystemExit) as stop:
            piv.pin_by_pinentry(piv.pinentry_program(), 7)
        self.assertIn("no PIN was given", str(stop.exception))

    def test_the_program_comes_from_the_environment(self):
        with mock.patch.dict(os.environ, {"DUKOVO_PINENTRY": "1"}):
            self.assertEqual(piv.pinentry_program(), "pinentry")
        with mock.patch.dict(os.environ, {"DUKOVO_PINENTRY": ""}):
            self.assertEqual(piv.pinentry_program(), "pinentry")
        with mock.patch.dict(os.environ, {"DUKOVO_PINENTRY": "/usr/bin/pinentry-gtk-2"}):
            self.assertEqual(piv.pinentry_program(), "/usr/bin/pinentry-gtk-2")

    def sign(self, pins):
        key = FakeYubiKey()
        out, err = io.StringIO(), io.StringIO()
        stdin = types.SimpleNamespace(buffer=io.BytesIO(b"message"))
        with self.environment(pins), mock.patch.object(piv, "yubikit", key.namespace), \
                mock.patch.object(sys, "stdin", stdin), redirect_stdout(out), redirect_stderr(err):
            piv.sign(7)
        return key, out.getvalue(), err.getvalue()

    def test_signing_asks_again_after_a_wrong_pin(self):
        key, out, err = self.sign(["654321", "123456"])
        self.assertEqual(key.pins, ["654321", "123456"])
        self.assertEqual(out.strip(), "30016d")
        self.assertIn("Touch YubiKey 7 now.", err)
        self.assertIn("SETERROR Wrong PIN: 2 tries left.", self.sent())

    def test_a_pin_of_the_wrong_length_costs_no_try(self):
        key, out, _ = self.sign(["12345", "123456789", "123456"])
        self.assertEqual(key.pins, ["123456"])
        self.assertEqual(out.strip(), "30016d")
        self.assertEqual(self.sent().count("SETERROR A YubiKey's PIN has 6 to 8 characters."), 2)

    def test_signing_stops_when_the_key_has_no_tries_left(self):
        with self.assertRaises(SystemExit) as stop:
            self.sign(["000000"])
        self.assertIn("0 tries left", str(stop.exception))
        self.assertIn("SETERROR Wrong PIN: 1 try left.", self.sent())


if __name__ == "__main__":
    unittest.main()
