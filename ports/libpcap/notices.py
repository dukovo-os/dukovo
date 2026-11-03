# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""Writes the notices of the source files a C build compiled, as they are.

Usage: python3 -I notices.py SOURCE BUILD NAME LICENSE [FILE...]

BUILD holds the dependency files (.d) the compiler wrote beside each
object with -MD. Each file under SOURCE they name, and each FILE (a path
in SOURCE: a grammar, say, that a generated source was made from), is
read for its notices: the comments before its first line of code that
hold a copyright or a license, and any comment after that holds a
copyright, whether in /* */ or in lines of //. Each notice is written
once, as the file has it without the comment's marks, after the names of
the files it is in, in the order of their names, after the project's
LICENSE (a path in SOURCE), the license of the files without one of
their own. The text goes to standard output, for
image/licenses/LICENSE-LicenseRef-NAME-notices.txt (ADR 0346).
"""

import os
import re
import sys

COMMENT = re.compile(r"/\*(.*?)\*/", re.S)
NOTICE = re.compile(
    r"copyright|redistribution|permission to use|public domain|license|licence|"
    r"may be (freely )?(distributed|used)",
    re.I,
)
COPYRIGHT = re.compile(r"copyright\s*(\(c\)|\d|©)", re.I)
# A run of // comment lines.
LINES = re.compile(r"(?:^[ \t]*//[^\n]*\n?)+", re.M)


def built(source, build):
    """The files under SOURCE the dependency files in BUILD name."""
    source = os.path.realpath(source)
    found = set()
    for top, _, names in os.walk(build):
        for name in names:
            if not name.endswith(".d"):
                continue
            with open(os.path.join(top, name), errors="replace") as f:
                text = f.read().replace("\\\n", " ")
            for line in text.splitlines():
                _, _, needs = line.partition(":")
                for path in needs.split():
                    path = os.path.realpath(os.path.join(top, path))
                    if path.startswith(source + os.sep):
                        found.add(os.path.relpath(path, source))
    return found


def unmark(comment):
    """A comment's text without its marks: the `*` that starts its lines."""
    lines = comment.split("\n")
    out = []
    for line in lines:
        line = line.rstrip()
        line = re.sub(r"^\s*\*+ ?", "", line) if re.match(r"^\s*\*", line) else line.strip()
        out.append(line)
    while out and not out[0].strip():
        out.pop(0)
    while out and not out[-1].strip():
        out.pop()
    return "\n".join(out)


def notices(path):
    """The notices of the file at PATH."""
    with open(path, errors="replace") as f:
        text = f.read()
    # Where its code starts: the first line that is not a comment, a
    # preprocessor line or blank, outside the comments.
    code = len(text)
    stripped = COMMENT.sub(lambda m: re.sub(r"[^\n]", " ", m.group(0)), text)
    offset = 0
    for line in stripped.splitlines(keepends=True):
        if line.strip() and not line.lstrip().startswith("#") and not line.lstrip().startswith("//"):
            code = offset
            break
        offset += len(line)
    found = []
    for m in COMMENT.finditer(text):
        body = m.group(1)
        if (m.start() < code and NOTICE.search(body)) or COPYRIGHT.search(body):
            found.append((m.start(), unmark(body)))
    for m in LINES.finditer(stripped):
        body = "\n".join(re.sub(r"^\s*// ?", "", line) for line in m.group(0).splitlines())
        if (m.start() < code and NOTICE.search(body)) or COPYRIGHT.search(body):
            found.append((m.start(), unmark(body)))
    return [notice for _, notice in sorted(found)]


def main():
    source, build, name, license = sys.argv[1:5]
    files = built(source, build) | set(sys.argv[5:])
    by_notice = {}
    for rel in sorted(files):
        for notice in notices(os.path.join(source, rel)):
            by_notice.setdefault(notice, []).append(rel)
    out = sys.stdout
    out.write(
        f"The notices of the source files of {name} that Dukovo's build\n"
        "compiles, as the files have them, each after the names of the files\n"
        "it is in. The licenses the SPDX License List names that the manifest\n"
        "gives beside this text are of some of these notices; this text keeps\n"
        "every one, with its copyright lines.\n"
    )
    with open(os.path.join(source, license), errors="replace") as f:
        out.write("\n" + "=" * 72 + "\n")
        out.write(f"{license}: the license of the files without a notice of their own\n\n")
        out.write(f.read().rstrip("\n") + "\n")
    for notice, rels in sorted(by_notice.items(), key=lambda item: item[1]):
        out.write("\n" + "=" * 72 + "\n")
        out.write("Files: " + " ".join(rels) + "\n\n")
        out.write(notice + "\n")


if __name__ == "__main__":
    main()
