#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""Reads what testssl.sh found about the web console's TLS (ADR 0514) and says whether it is as it must be.

    testssl-check.py FINDINGS ACCEPTED

FINDINGS is testssl.sh's flat JSON file (--jsonfile): a list of findings, each
with an id, a severity and a finding. ACCEPTED is testssl-accepted.txt: a
finding's id, white space and the reason it is no fault. It prints a line
for each thing that holds, and "FAILED:" lines and status 1 for each that
does not:

    testssl: TLS 1.2 and 1.3 only
    testssl: forward secrecy only
    testssl: no weak cipher
    testssl: the certificate's names and dates are right
    testssl: nothing of MEDIUM or worse that testssl-accepted.txt does not list

The checks, by what testssl.sh names them:

- SSLv2, SSLv3, TLS1 and TLS1_1 are not offered, and TLS1_2 and TLS1_3 are;
- every cipher that testssl.sh finds for a protocol (`cipher-PROTOCOL_HEX`)
  is a forward-secret one: a TLS 1.3 suite, or one whose name has DHE in it,
  which ECDHE has too, and `cipherlist_STRONG_NOFS`, the strong ciphers
  with no forward secrecy, is not offered;
- no cipher list of NULL, anonymous, export, LOW, 3DES and IDEA ciphers is
  offered;
- the certificate names localhost, which the console takes for its own
  name, and is valid now;
- every finding of severity MEDIUM, HIGH, CRITICAL or WARN has its id in
  the accepted file. An id there that testssl.sh did not report is
  said, and no fault.
"""

import json
import re
import sys
from datetime import datetime, timezone

NOT_ACCEPTED = {"MEDIUM", "HIGH", "CRITICAL", "WARN"}
NEVER = ("NULL", "aNULL", "EXPORT", "LOW", "3DES_IDEA")


def plain_id(finding_id):
    """The id without what testssl.sh adds for a second certificate, ` <hostCert#2>`."""
    return finding_id.split(" ")[0]


def accepted_ids(text):
    """The ids of testssl-accepted.txt, each with its reason; a line without a reason is a mistake."""
    accepted = {}
    for number, line in enumerate(text.splitlines(), start=1):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split(None, 1)
        finding_id, reason = parts[0], parts[1].strip() if len(parts) > 1 else ""
        if not reason:
            raise ValueError(f"line {number} of testssl-accepted.txt has an id and no reason")
        accepted[finding_id] = reason
    return accepted


def by_id(findings):
    """The findings by their plain id; a second certificate's come after the first's."""
    found = {}
    for each in findings:
        found.setdefault(plain_id(each["id"]), []).append(each)
    return found


def forward_secret(protocol, name):
    """Whether a cipher testssl.sh found is forward-secret: every TLS 1.3 suite is, and TLS 1.2's that have DHE in the name."""
    return protocol == "TLSv1.3" or "DHE" in name


def cipher_problems(findings):
    """The ciphers that are not forward-secret, as sentences."""
    problems = []
    for each in findings:
        if not re.match(r"cipher-tls[0-9_]+_x[0-9a-f]+", plain_id(each["id"])):
            continue
        words = each["finding"].split()
        # "TLSv1.2  xc030   ECDHE-RSA-AES256-GCM-SHA384   ECDH 521 ..."
        if len(words) < 3:
            problems.append(f"{each['id']} says {each['finding']!r}, which does not read")
        elif not forward_secret(words[0], words[2]):
            problems.append(f"{words[0]} offers {words[2]}, which has no forward secrecy")
    return problems


def date_of(text):
    """The UTC time of testssl.sh's `2026-11-13 20:11`, or None."""
    found = re.search(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}", text)
    return datetime.strptime(found.group(0), "%Y-%m-%d %H:%M").replace(tzinfo=timezone.utc) if found else None


def check(findings, accepted, now):
    """What is wrong with `findings`, as sentences, and the lines for what is right."""
    found = by_id(findings)
    problems, said = [], []

    def finding(finding_id):
        return found.get(finding_id, [{"finding": "", "severity": ""}])[0]

    protocols = []
    for finding_id in ("SSLv2", "SSLv3", "TLS1", "TLS1_1"):
        text = finding(finding_id)["finding"]
        if not text.startswith(("not offered", "likely not offered")):
            protocols.append(f"{finding_id} is {text or 'not reported'}, not not offered")
    for finding_id in ("TLS1_2", "TLS1_3"):
        text = finding(finding_id)["finding"]
        if not text.startswith("offered"):
            protocols.append(f"{finding_id} is {text or 'not reported'}, not offered")
    problems += protocols
    if not protocols:
        said.append("TLS 1.2 and 1.3 only")

    secrecy = cipher_problems(findings)
    if not any(plain_id(each["id"]).startswith("cipher-") for each in findings):
        secrecy.append("testssl.sh reported no cipher of any protocol")
    if finding("cipherlist_STRONG_NOFS")["finding"] != "not offered":
        secrecy.append(f"cipherlist_STRONG_NOFS is {finding('cipherlist_STRONG_NOFS')['finding'] or 'not reported'}, not not offered")
    problems += secrecy
    if not secrecy:
        said.append("forward secrecy only")

    weak = [
        f"cipherlist_{name} is {finding('cipherlist_' + name)['finding'] or 'not reported'}, not not offered"
        for name in NEVER
        if finding("cipherlist_" + name)["finding"] != "not offered"
    ]
    problems += weak
    if not weak:
        said.append("no weak cipher")

    certificate = []
    if "localhost" not in finding("cert_subjectAltName")["finding"]:
        certificate.append(f"the certificate's names are {finding('cert_subjectAltName')['finding'] or 'not reported'}, and do not hold localhost")
    before, after = date_of(finding("cert_notBefore")["finding"]), date_of(finding("cert_notAfter")["finding"])
    if before is None or after is None:
        certificate.append("testssl.sh gave no dates for the certificate")
    elif not before <= now <= after:
        certificate.append(f"the certificate is valid from {before} to {after}, and it is {now}")
    problems += certificate
    if not certificate:
        said.append("the certificate's names and dates are right")

    unlisted = sorted(
        {
            plain_id(each["id"])
            for each in findings
            if each["severity"] in NOT_ACCEPTED and plain_id(each["id"]) not in accepted
        }
    )
    for finding_id in unlisted:
        each = found[finding_id][0]
        problems.append(f"{finding_id} is {each['severity']}: {each['finding']} (not in testssl-accepted.txt)")
    if not unlisted:
        said.append("nothing of MEDIUM or worse that testssl-accepted.txt does not list")
    return problems, said


def main():
    findings_file, accepted_file = sys.argv[1:3]
    with open(findings_file) as file:
        findings = json.load(file)
    if not isinstance(findings, list) or not findings:
        print("FAILED: testssl.sh's file holds no findings")
        return 1
    with open(accepted_file) as file:
        accepted = accepted_ids(file.read())
    problems, said = check(findings, accepted, datetime.now(timezone.utc))
    for line in said:
        print(f"testssl: {line}")
    for problem in problems:
        print(f"FAILED: {problem}")
    seen = {plain_id(each["id"]) for each in findings}
    for finding_id in sorted(set(accepted) - seen):
        print(f"note: {finding_id} is in testssl-accepted.txt and testssl.sh did not report it")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
